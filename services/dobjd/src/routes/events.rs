use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::State,
    response::sse::{Event as SseEvent, KeepAlive, Sse},
};
use futures_util::stream::{Stream, StreamExt};
use tokio_stream::wrappers::BroadcastStream;

use crate::state::AppState;

/// Stream events with the sequence number in both SSE `id` and JSON `seq`.
/// Missing sequence numbers indicate dropped events; reload state through REST.
pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.services.events.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|item| async move {
        match item {
            Ok(envelope) => {
                let json = serde_json::to_string(&envelope)
                    .expect("Envelope must always serialize to JSON");
                Some(Ok(SseEvent::default()
                    .id(envelope.seq.to_string())
                    .data(json)))
            }
            Err(_) => None,
        }
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
