//! Maps `Box<dyn Error>` from trait boundaries to typed `DoserError`.
//!
//! The traits in `doser_traits` use `Box<dyn Error + Send + Sync>` for maximum
//! flexibility; this module converts those to our typed error enum, with an
//! optional feature-gated path for `doser_hardware::HwError` downcasting.

use crate::error::DoserError;

/// Map a trait-boundary error to a typed `DoserError`.
///
/// Attempts to downcast known hardware error types first, then falls back
/// to string-based heuristics.
pub fn map_hw_error(e: &(dyn std::error::Error + 'static)) -> DoserError {
    // Feature-gated: try to downcast to HwError for precise mapping
    #[cfg(feature = "hardware-errors")]
    {
        if let Some(hw) = e.downcast_ref::<doser_hardware::error::HwError>() {
            return match hw {
                doser_hardware::error::HwError::Timeout => DoserError::Timeout,
                doser_hardware::error::HwError::DataReadyTimeout => DoserError::Timeout,
                other => DoserError::HardwareFault(other.to_string()),
            };
        }
    }

    // Fallback: string-based detection
    let s = e.to_string();
    if s.to_lowercase().contains("timeout") {
        DoserError::Timeout
    } else {
        DoserError::Hardware(s)
    }
}
