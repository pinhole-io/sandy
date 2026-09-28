use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FirecrackerError {
    #[error("Firecracker is only supported on Linux")]
    UnsupportedPlatform,

    #[error("Firecracker is unavailable: {0}")]
    Unavailable(String),

    #[error("invalid Firecracker configuration: {0}")]
    InvalidConfig(String),

    #[error("Firecracker API request failed: {0}")]
    Api(String),

    #[error("Firecracker guest agent failed: {0}")]
    Guest(String),

    #[error("Firecracker execution timed out after {0} seconds")]
    TimedOut(u64),

    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}
