pub mod config;
/// Builder for a dosing session
#[derive(Debug, Default, Clone)]
pub struct DosingSessionBuilder {
    target_grams: Option<f32>,
    max_attempts: Option<usize>,
    dt_pin: Option<u8>,
    sck_pin: Option<u8>,
    step_pin: Option<u8>,
    dir_pin: Option<u8>,
}

impl DosingSessionBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn target_grams(mut self, grams: f32) -> Self {
        self.target_grams = Some(grams);
        self
    }
    pub fn max_attempts(mut self, attempts: usize) -> Self {
        self.max_attempts = Some(attempts);
        self
    }
    pub fn dt_pin(mut self, pin: u8) -> Self {
        self.dt_pin = Some(pin);
        self
    }
    pub fn sck_pin(mut self, pin: u8) -> Self {
        self.sck_pin = Some(pin);
        self
    }
    pub fn step_pin(mut self, pin: u8) -> Self {
        self.step_pin = Some(pin);
        self
    }
    pub fn dir_pin(mut self, pin: u8) -> Self {
        self.dir_pin = Some(pin);
        self
    }
    pub fn build(self) -> Option<DosingSession> {
        Some(DosingSession {
            target_grams: self.target_grams?,
            max_attempts: self.max_attempts.unwrap_or(100),
            dt_pin: self.dt_pin?,
            sck_pin: self.sck_pin?,
            step_pin: self.step_pin?,
            dir_pin: self.dir_pin?,
        })
    }
}

/// ADT for a dosing session
#[derive(Debug, Clone, PartialEq)]
pub struct DosingSession {
    pub target_grams: f32,
    pub max_attempts: usize,
    pub dt_pin: u8,
    pub sck_pin: u8,
    pub step_pin: u8,
    pub dir_pin: u8,
}

impl std::fmt::Display for DosingSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DosingSession(target_grams: {:.2}, max_attempts: {}, DT: {}, SCK: {}, STEP: {}, DIR: {})",
            self.target_grams,
            self.max_attempts,
            self.dt_pin,
            self.sck_pin,
            self.step_pin,
            self.dir_pin
        )
    }
}
impl DosingSession {
    /// Returns a dosing step iterator for the session
    pub fn steps<F>(&self, read_weight: F) -> DosingStepEnum<F>
    where
        F: FnMut() -> f32,
    {
        DosingStepEnum::Steps(DosingStep::new(
            self.target_grams,
            read_weight,
            self.max_attempts,
        ))
    }
}

/// ADT for dosing step iterator
pub enum DosingStepEnum<F>
where
    F: FnMut() -> f32,
{
    Steps(DosingStep<F>),
}

impl<F> Iterator for DosingStepEnum<F>
where
    F: FnMut() -> f32,
{
    type Item = (usize, f32);
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            DosingStepEnum::Steps(iter) => iter.next(),
        }
    }
}
use std::fmt;
use thiserror::Error;

/// Error types for dosing operations
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DoserError {
    /// Target weight must be positive
    #[error("Negative target grams")]
    NegativeTarget,
    /// Maximum number of dosing attempts exceeded
    #[error("Max attempts exceeded")]
    MaxAttemptsExceeded,
    /// Weight reading was negative
    #[error("Negative weight reading")]
    NegativeWeight,
}
impl fmt::Display for DosingResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Final weight: {:.2}g, Attempts: {}, Error: {}",
            self.final_weight,
            self.attempts,
            match &self.error {
                Some(e) => e.to_string(),
                None => "None".to_string(),
            }
        )
    }
}
use chrono::Local;
use std::fs::OpenOptions;
use std::io::Write;

/// Log dosing result to a file, including timestamp, pin config, and calibration info
pub fn log_dosing_result(
    path: &str,
    result: &DosingResult,
    target: f32,
    dt_pin: u8,
    sck_pin: u8,
    step_pin: u8,
    dir_pin: u8,
    calibration: Option<f32>,
) {
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_entry = format!(
        "{} | target: {:.2}, final: {:.2}, attempts: {}, error: {:?}, pins: DT={}, SCK={}, STEP={}, DIR={}, calibration: {:?}\n",
        timestamp,
        target,
        result.final_weight,
        result.attempts,
        result.error,
        dt_pin,
        sck_pin,
        step_pin,
        dir_pin,
        calibration
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(log_entry.as_bytes());
    }
}
/// Render a simple progress bar for dosing
pub fn render_progress_bar(current: f32, target: f32, bar_width: usize) -> String {
    let percent = (current / target * 100.0).min(100.0).max(0.0);
    let filled = ((percent / 100.0) * bar_width as f32).round() as usize;
    let empty = bar_width - filled;
    let bar = format!("[{}{}]", "#".repeat(filled), "-".repeat(empty));
    format!(
        "{} {:>5.1}% ({:.2}g / {:.2}g)",
        bar, percent, current, target
    )
}
// ...existing code...

/// Dosing result
#[derive(Debug, Clone, PartialEq)]
pub struct DosingResult {
    pub final_weight: f32,
    pub attempts: usize,
    pub error: Option<DoserError>,
}

impl Default for DosingResult {
    fn default() -> Self {
        DosingResult {
            final_weight: 0.0,
            attempts: 0,
            error: None,
        }
    }
}

/// Core dosing algorithm (hardware-agnostic)
/// Attempts to dose to the target weight.
///
/// # Errors
/// Returns `Err(DoserError)` if:
/// - `target_grams` is not positive
/// - `read_weight` closure returns a negative value
/// - Maximum attempts are exceeded before reaching target
///
/// # Returns
/// - Ok(DosingResult) if successful
/// - Err(DoserError) if a precondition or runtime error occurs
pub fn dose_to_target<F>(
    target_grams: f32,
    mut read_weight: F,
    max_attempts: usize,
) -> Result<DosingResult, DoserError>
where
    F: FnMut() -> f32,
{
    if target_grams <= 0.0 {
        return Err(DoserError::NegativeTarget);
    }
    let mut attempts = 0;
    let mut current_weight = read_weight();
    while current_weight < target_grams {
        attempts += 1;
        if attempts > max_attempts {
            return Err(DoserError::MaxAttemptsExceeded);
        }
        if current_weight < 0.0 {
            return Err(DoserError::NegativeWeight);
        }
        current_weight = read_weight();
    }
    Ok(DosingResult {
        final_weight: current_weight,
        attempts,
        error: None,
    })
}

/// Calibration math (returns scale factor)
pub fn calculate_scale_factor(known_weight: f32, raw: f32) -> f32 {
    if raw > 0.0 { known_weight / raw } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(10.0, 2.0, 5.0)]
    #[case(10.0, 0.0, 1.0)]
    #[case(0.0, 10.0, 0.0)]
    fn test_calculate_scale_factor(#[case] known: f32, #[case] raw: f32, #[case] expected: f32) {
        assert_eq!(calculate_scale_factor(known, raw), expected);
    }

    #[test]
    fn test_dose_to_target_success() {
        let mut w = 0.0;
        let result = dose_to_target(
            10.0,
            || {
                w += 2.0;
                w
            },
            10,
        );
        assert!(result.is_ok());
        let dosing = result.unwrap();
        assert_eq!(dosing.final_weight, 10.0);
        assert_eq!(dosing.error, None);
    }

    #[test]
    fn test_dose_to_target_negative_target() {
        let result = dose_to_target(-5.0, || 0.0, 10);
        assert_eq!(result, Err(DoserError::NegativeTarget));
    }

    #[test]
    fn test_dose_to_target_max_attempts() {
        let mut w = 0.0;
        let result = dose_to_target(
            10.0,
            || {
                w += 1.0;
                w
            },
            5,
        );
        assert_eq!(result, Err(DoserError::MaxAttemptsExceeded));
    }

    #[test]
    fn test_dose_to_target_negative_weight() {
        let mut w = 0.0;
        let result = dose_to_target(
            10.0,
            || {
                w -= 2.0;
                w
            },
            10,
        );
        assert_eq!(result, Err(DoserError::NegativeWeight));
    }
}
/// Iterator over dosing steps, yielding (attempt, weight) each time.
pub struct DosingStep<F>
where
    F: FnMut() -> f32,
{
    target: f32,
    max_attempts: usize,
    read_weight: F,
    attempt: usize,
    finished: bool,
}

impl<F> DosingStep<F>
where
    F: FnMut() -> f32,
{
    pub fn new(target: f32, read_weight: F, max_attempts: usize) -> Self {
        DosingStep {
            target,
            max_attempts,
            read_weight,
            attempt: 0,
            finished: false,
        }
    }
}

impl<F> Iterator for DosingStep<F>
where
    F: FnMut() -> f32,
{
    type Item = (usize, f32);
    fn next(&mut self) -> Option<Self::Item> {
        if self.finished || self.attempt >= self.max_attempts {
            return None;
        }
        self.attempt += 1;
        let weight = (self.read_weight)();
        if weight >= self.target || weight < 0.0 {
            self.finished = true;
        }
        Some((self.attempt, weight))
    }
}

#[cfg(test)]
mod iterator_tests {
    use super::*;
    #[test]
    fn test_dosing_step_iterator() {
        let mut w = 0.0;
        let iter = DosingStep::new(
            6.0,
            || {
                w += 2.0;
                w
            },
            10,
        );
        let steps: Vec<_> = iter.collect();
        assert_eq!(steps, vec![(1, 2.0), (2, 4.0), (3, 6.0)]);
    }

    #[test]
    fn test_dosing_step_iterator_negative_weight() {
        let mut w = 2.0;
        let iter = DosingStep::new(
            6.0,
            || {
                w -= 3.0;
                w
            },
            10,
        );
        let steps: Vec<_> = iter.collect();
        // Should stop after first negative weight
        assert_eq!(steps, vec![(1, -1.0)]);
    }

    #[test]
    fn test_dosing_step_iterator_max_attempts() {
        let mut w = 0.0;
        let iter = DosingStep::new(
            100.0,
            || {
                w += 1.0;
                w
            },
            3,
        );
        let steps: Vec<_> = iter.collect();
        assert_eq!(steps, vec![(1, 1.0), (2, 2.0), (3, 3.0)]);
    }

    #[test]
    fn test_dosing_step_enum_iterator() {
        let mut w = 0.0;
        let mut iter = DosingStepEnum::Steps(DosingStep::new(
            5.0,
            || {
                w += 2.0;
                w
            },
            10,
        ));
        let mut results = vec![];
        while let Some((attempt, weight)) = iter.next() {
            results.push((attempt, weight));
        }
        assert_eq!(results, vec![(1, 2.0), (2, 4.0), (3, 6.0)]);
    }

    #[test]
    fn test_dosing_session_builder_and_steps() {
        let builder = DosingSessionBuilder::new()
            .target_grams(6.0)
            .max_attempts(5)
            .dt_pin(1)
            .sck_pin(2)
            .step_pin(3)
            .dir_pin(4);
        let session = builder.build().unwrap();
        let mut w = 0.0;
        let mut iter = session.steps(|| {
            w += 2.0;
            w
        });
        let mut results = vec![];
        while let Some((attempt, weight)) = iter.next() {
            results.push((attempt, weight));
        }
        assert_eq!(results, vec![(1, 2.0), (2, 4.0), (3, 6.0)]);
    }

    #[test]
    fn test_dosing_session_builder_missing_fields() {
        let builder = DosingSessionBuilder::new()
            .target_grams(6.0)
            .max_attempts(5)
            .dt_pin(1)
            .sck_pin(2)
            .step_pin(3);
        // Missing dir_pin
        assert!(builder.build().is_none());
    }
}
