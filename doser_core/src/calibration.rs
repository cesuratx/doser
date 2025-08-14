pub trait CalibrationSource {
    fn get_calibration(&self) -> Vec<(f32, f32)>;
}

pub struct CsvCalibrationSource {
    data: Vec<(f32, f32)>,
}

impl CsvCalibrationSource {
    pub fn new(data: Vec<(f32, f32)>) -> Self {
        Self { data }
    }
}

impl CalibrationSource for CsvCalibrationSource {
    fn get_calibration(&self) -> Vec<(f32, f32)> {
        self.data.clone()
    }
}
