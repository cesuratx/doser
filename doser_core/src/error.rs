//! Domain and build errors for the dosing engine, plus a stable `AbortReason` enum
//! used by the CLI to map to exit codes and JSON fields.
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AbortReason {
    Estop,
    NoProgress,
    MaxRuntime,
    Overshoot,
    MaxAttempts,
}

impl core::fmt::Display for AbortReason {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AbortReason::Estop => write!(f, "estop"),
            AbortReason::NoProgress => write!(f, "no progress"),
            AbortReason::MaxRuntime => write!(f, "max run time exceeded"),
            AbortReason::Overshoot => write!(f, "max overshoot exceeded"),
            AbortReason::MaxAttempts => write!(f, "max attempts exceeded"),
        }
    }
}

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
    #[error("aborted: {0}")]
    Abort(AbortReason),
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

#[cfg(test)]
mod tests {
    use super::AbortReason::*;
    #[test]
    fn abort_reason_display_is_stable() {
        assert_eq!(Estop.to_string(), "estop");
        assert_eq!(NoProgress.to_string(), "no progress");
        assert_eq!(MaxRuntime.to_string(), "max run time exceeded");
        assert_eq!(Overshoot.to_string(), "max overshoot exceeded");
        assert_eq!(MaxAttempts.to_string(), "max attempts exceeded");
    }
}
