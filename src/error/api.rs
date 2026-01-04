use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid base url: {0}")]
    Url(String),
    #[error("http error: {0}")]
    Http(String),
    #[error("decode error: {0}")]
    Decode(String),
}
