use std::future::Future;
use std::time::Duration;

use anyhow::Result;
use tokio::time::Instant;
use wire_types::WaitQuery;

use crate::events::{Event, EventHub};

/// Maximum wait duration before returning the current state.
pub const MAX_WAIT: Duration = Duration::from_secs(120);
pub const DEFAULT_WAIT: Duration = Duration::from_secs(30);

/// Read immediately or wait for completion, bounded by the query timeout.
pub async fn wait_until<T, Read, ReadFuture, Done, Match>(
    events: &EventHub,
    query: WaitQuery,
    read: Read,
    is_done: Done,
    from_event: Match,
) -> Result<T>
where
    Read: Fn() -> ReadFuture,
    ReadFuture: Future<Output = Result<T>>,
    Done: Fn(&T) -> bool,
    Match: Fn(&Event) -> Result<Option<T>>,
{
    if !query.wait {
        return read().await;
    }
    let timeout = query
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_WAIT)
        .min(MAX_WAIT);
    follow(
        events,
        Some(Instant::now() + timeout),
        read,
        is_done,
        from_event,
    )
    .await
}

/// Subscribe before reading to avoid missing a concurrent change.
/// Re-read after lag or at the deadline; propagate matching event errors.
pub async fn follow<T, Read, ReadFuture, Done, Match>(
    events: &EventHub,
    deadline: Option<Instant>,
    read: Read,
    is_done: Done,
    from_event: Match,
) -> Result<T>
where
    Read: Fn() -> ReadFuture,
    ReadFuture: Future<Output = Result<T>>,
    Done: Fn(&T) -> bool,
    Match: Fn(&Event) -> Result<Option<T>>,
{
    let mut rx = events.subscribe();
    let mut current = read().await?;
    while !is_done(&current) {
        let received = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(received) => received,
                Err(_elapsed) => return read().await,
            },
            None => rx.recv().await,
        };
        match received {
            Ok(envelope) => {
                if let Some(state) = from_event(&envelope.event)? {
                    current = state;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                current = read().await?;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                anyhow::bail!("event hub closed");
            }
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn removed(file_name: &str) -> Event {
        Event::ObjectRemoved {
            file_name: file_name.to_string(),
        }
    }

    #[tokio::test]
    async fn returns_at_once_without_wait_and_follows_events_with_it() {
        let hub = EventHub::new();
        let reads = Arc::new(AtomicU32::new(0));
        let read = {
            let reads = reads.clone();
            move || {
                reads.fetch_add(1, Ordering::Relaxed);
                std::future::ready(Ok(0u32))
            }
        };
        let matched = |event: &Event| match event {
            Event::ObjectRemoved { file_name } if file_name == "wanted" => Ok(Some(1u32)),
            Event::ObjectRemoved { file_name } if file_name == "poison" => {
                anyhow::bail!("gone")
            }
            _ => Ok(None),
        };

        let value = wait_until(&hub, WaitQuery::default(), &read, |v| *v == 1, matched)
            .await
            .unwrap();
        assert_eq!(value, 0);

        let waiting = {
            let hub = hub.clone();
            let read = read.clone();
            tokio::spawn(async move {
                wait_until(
                    &hub,
                    WaitQuery {
                        wait: true,
                        timeout_ms: Some(5_000),
                    },
                    read,
                    |v| *v == 1,
                    matched,
                )
                .await
                .unwrap()
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        hub.publish(removed("other"));
        hub.publish(removed("wanted"));
        assert_eq!(waiting.await.unwrap(), 1);

        let timed_out = wait_until(
            &hub,
            WaitQuery {
                wait: true,
                timeout_ms: Some(20),
            },
            &read,
            |v| *v == 1,
            matched,
        )
        .await
        .unwrap();
        assert_eq!(timed_out, 0);
        assert_eq!(reads.load(Ordering::Relaxed), 4);

        let failing = {
            let hub = hub.clone();
            tokio::spawn(async move {
                follow(
                    &hub,
                    None,
                    || std::future::ready(Ok(0u32)),
                    |v| *v == 1,
                    matched,
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        hub.publish(removed("poison"));
        assert!(failing.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn events_during_an_async_read_are_preserved_or_reloaded_after_lag() {
        for lagged in [false, true] {
            let hub = EventHub::new();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let reads = Arc::new(AtomicU32::new(0));
            let read = {
                let entered = entered.clone();
                let release = release.clone();
                let reads = reads.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    let reads = reads.clone();
                    async move {
                        if reads.fetch_add(1, Ordering::Relaxed) == 0 {
                            entered.notify_one();
                            release.notified().await;
                            Ok(0u32)
                        } else {
                            Ok(2u32)
                        }
                    }
                }
            };
            let waiting = {
                let hub = hub.clone();
                tokio::spawn(async move {
                    follow(&hub, None, read, |value| *value > 0, |_| Ok(Some(1u32))).await
                })
            };
            entered.notified().await;
            let events = if lagged {
                crate::events::CHANNEL_CAPACITY + 1
            } else {
                1
            };
            for _ in 0..events {
                hub.publish(removed("changed"));
            }
            release.notify_one();
            let result = tokio::time::timeout(Duration::from_secs(1), waiting)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(result, if lagged { 2 } else { 1 });
            assert_eq!(reads.load(Ordering::Relaxed), if lagged { 2 } else { 1 });
        }
    }
}
