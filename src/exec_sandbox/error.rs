use thiserror::Error;

#[derive(Error, Debug)]
pub enum SandboxError {
    #[error("sandbox backend unavailable: {0}")]
    Unavailable(String),

    #[error("failed to spawn command: {0}")]
    SpawnFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("command timed out after {0} seconds")]
    TimedOut(u64),

    #[error("invalid policy: {0}")]
    InvalidPolicy(String),

    #[error("command output exceeded {0} bytes")]
    OutputLimitExceeded(usize),
}
