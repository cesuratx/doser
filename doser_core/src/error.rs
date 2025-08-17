use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum DoserError {
    #[error("hardware error: {0}")]
    Hardware(String),
    #[error("hardware fault: {0}")]
    HardwareFault(String),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("timeout waiting for sensor")]
    Timeout,
    #[error("invalid state: {0}")]
    State(String),
    #[error("io error: {0}")]
    Io(String),
}

pub type Result<T> = std::result::Result<T, DoserError>;
