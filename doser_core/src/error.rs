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

#[derive(Debug, Error, Clone)]
pub enum BuildError {
    #[error("missing scale")]
    MissingScale,
    #[error("missing motor")]
    MissingMotor,
    #[error("missing target grams")]
    MissingTarget,
    #[error("invalid config: {0}")]
    InvalidConfig(&'static str),
}

pub type Result<T> = eyre::Result<T>;
pub use eyre::Report;
