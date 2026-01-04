use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("failed to spawn opencode: {0}")]
    Spawn(String),
    #[error("failed to parse server url from output")]
    Parse,
    #[error("server did not become ready within timeout")]
    Timeout,
}
