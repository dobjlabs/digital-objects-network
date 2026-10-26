use std::time::Duration;

use anyhow::Result;
use wire_types::WaitQuery;

use crate::events::{Event, EventHub};

/// Maximum wait duration before returning the current state.
pub const MAX_WAIT: Duration = Duration::from_secs(120);
pub const DEFAULT_WAIT: Duration = Duration::from_secs(30);

/// Answers `read()` at once unless `query.wait` is set; then returns the
/// first state for which `is_done` holds, taken from `read()` or from a
/// matching event, or the current state at the timeout. Subscribes before
/// reading, so a change between the two is not missed.
pub async fn wait_until<T, Read, Done, Match>(
    events: &EventHub,
    query: WaitQuery,
    read: Read,
    is_done: Done,
    from_event: Match,
) -> Result<T>
where
    Read: Fn() -> Result<T>,
    Done: Fn(&T) -> bool,
    Match: Fn(&Event) -> Option<T>,
{
    if !query.wait {
        return read();
    }
    let timeout = query
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_WAIT)
        .min(MAX_WAIT);
    let mut rx = events.subscribe();
    let mut current = read()?;
    let deadline = tokio::time::Instant::now() + timeout;
    while !is_done(&current) {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(envelope)) => {
                if let Some(state) = from_event(&envelope.event) {
                    current = state;
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {
                current = read()?;
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
            Err(_elapsed) => {
                current = read()?;
                break;
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
                Ok(0u32)
            }
        };
        let matched = |event: &Event| match event {
            Event::ObjectRemoved { file_name } if file_name == "wanted" => Some(1u32),
            _ => None,
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
    }
}
