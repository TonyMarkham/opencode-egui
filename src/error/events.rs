use thiserror::Error;

#[derive(Debug, Error)]
pub enum EventsError {
    #[error("http error: {0}")]
    Http(String),
}
