use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::State,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
};
use futures_util::stream::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use crate::state::AppState;

/// SSE endpoint streaming server events to the frontend.
///
/// Uses a broadcast subscription per connection. If the subscriber lags
/// behind the channel buffer we just skip the dropped events — `EventSource`
/// auto-reconnects, and on reconnect the client refetches state via the
/// regular REST routes.
pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| async move {
        match item {
            // `Event` is a closed enum of structs whose fields are all
            // primitives (`String`, `Option`, `Vec<String>`, enums with
            // derived `Serialize`) — serialization can't fail here, so a
            // failure means a future variant violated that invariant and
            // we want to know loudly.
            Ok(event) => {
                let json =
                    serde_json::to_string(&event).expect("Event must always serialize to JSON");
                Some(Ok(SseEvent::default().data(json)))
            }
            // Lagged subscriber — silently drop, browser will keep reading.
            Err(_) => None,
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
