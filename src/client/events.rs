use crate::error::events::EventsError;
use futures_util::stream::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use tokio::sync::mpsc;

#[derive(Debug, Deserialize, Clone)]
pub struct GlobalEvent {
    pub directory: String,
    pub payload: serde_json::Value,
}

/// Start an SSE subscription to /global/event and return a receiver of parsed GlobalEvent.
pub async fn subscribe_global(base_url: &str) -> Result<mpsc::Receiver<GlobalEvent>, EventsError> {
    let url = format!("{}/global/event", base_url.trim_end_matches('/'));
    let mut es = EventSource::get(url);
    let (tx, rx) = mpsc::channel(256);

    tokio::spawn(async move {
        loop {
            match es.next().await {
                Some(Ok(Event::Open)) => {}
                Some(Ok(Event::Message(message))) => {
                    if let Ok(ev) = serde_json::from_str::<GlobalEvent>(&message.data) {
                        let _ = tx.send(ev).await;
                    }
                }
                Some(Err(_)) | None => {
                    break;
                }
            }
        }
    });

    Ok(rx)
}
