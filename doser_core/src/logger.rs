pub trait Logger {
    fn log(&self, message: &str);
}

pub struct FileLogger {
    path: String,
}

impl FileLogger {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

impl Logger for FileLogger {
    fn log(&self, message: &str) {
        use std::fs::OpenOptions;
        use std::io::Write;
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{}", message);
        }
    }
}
