pub enum DosingMode {
    Normal,
    Calibration,
    Cleaning,
}

pub enum HardwareState {
    Idle,
    Running,
    Error(String),
}

pub struct LogEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event: String,
    pub details: Option<String>,
}
