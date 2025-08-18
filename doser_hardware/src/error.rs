use thiserror::Error;

#[derive(Debug, Error)]
pub enum HwError {
    #[error("gpio error: {0}")]
    Gpio(String),
    #[error("scale timeout")]
    Timeout,
    #[error("hx711 data-ready timeout")]
    DataReadyTimeout,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, HwError>;
