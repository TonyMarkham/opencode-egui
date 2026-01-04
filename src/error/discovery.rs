use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("failed to query system processes: {0}")]
    SystemQuery(String),
    #[error("failed to query network sockets: {0}")]
    NetworkQuery(String),
}
