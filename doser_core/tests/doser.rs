use doser_core::*;
use std::sync::{Arc, Mutex};

struct MockScale {
    weights: Arc<Mutex<Vec<f32>>>,
}
impl doser_hardware::Scale for MockScale {
    fn read(
        &mut self,
        _timeout: std::time::Duration,
    ) -> Result<i32, Box<dyn std::error::Error + Send + Sync>> {
        let mut w = self.weights.lock().unwrap();
        Ok(w.remove(0) as i32)
    }
}

struct MockMotor {
    started: Arc<Mutex<bool>>,
    stopped: Arc<Mutex<bool>>,
}
impl doser_hardware::Motor for MockMotor {
    fn set_speed(
        &mut self,
        _steps_per_sec: u32,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        *self.started.lock().unwrap() = true;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        *self.stopped.lock().unwrap() = true;
        Ok(())
    }
    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        *self.started.lock().unwrap() = true;
        Ok(())
    }
}

#[test]
fn test_doser_stepwise_dosing_complete() {
    let weights = Arc::new(Mutex::new(vec![1.0, 2.0, 3.0, 5.0]));
    let scale = MockScale {
        weights: weights.clone(),
    };
    let sampler =
        doser_core::sampler::Sampler::spawn(scale, 10, std::time::Duration::from_millis(100));
    let motor = Box::new(MockMotor {
        started: Arc::new(Mutex::new(false)),
        stopped: Arc::new(Mutex::new(false)),
    });
    let mut doser = Doser::new(sampler, motor, 4.0, 2, 1000);
    let mut status = doser.step().unwrap();
    match status {
        DosingStatus::Running => {}
        _ => panic!("Expected Running"),
    }
    status = doser.step().unwrap();
    match status {
        DosingStatus::Running => {}
        _ => panic!("Expected Running"),
    }
    status = doser.step().unwrap();
    match status {
        DosingStatus::Running => {}
        _ => panic!("Expected Running"),
    }
    status = doser.step().unwrap();
    match status {
        DosingStatus::Complete => {}
        _ => panic!("Expected Complete"),
    }
}

#[test]
fn test_doser_filtered_weight() {
    let weights = Arc::new(Mutex::new(vec![2.0, 4.0, 6.0]));
    let scale = MockScale {
        weights: weights.clone(),
    };
    let sampler =
        doser_core::sampler::Sampler::spawn(scale, 10, std::time::Duration::from_millis(100));
    let motor = Box::new(MockMotor {
        started: Arc::new(Mutex::new(false)),
        stopped: Arc::new(Mutex::new(false)),
    });
    let mut doser = Doser::new(sampler, motor, 10.0, 2, 1000);
    doser.step().unwrap();
    doser.step().unwrap();
    assert!((doser.filtered_weight() - 3.0).abs() < 1e-6);
}

#[test]
fn test_doser_debug_display() {
    let weights = Arc::new(Mutex::new(vec![1.0, 2.0]));
    let scale = MockScale {
        weights: weights.clone(),
    };
    let sampler =
        doser_core::sampler::Sampler::spawn(scale, 10, std::time::Duration::from_millis(100));
    let motor = Box::new(MockMotor {
        started: Arc::new(Mutex::new(false)),
        stopped: Arc::new(Mutex::new(false)),
    });
    let doser = Doser::new(sampler, motor, 2.0, 2, 1000);
    let debug_str = format!("{:?}", doser);
    let display_str = format!("{}", doser);
    assert!(debug_str.contains("Doser"));
    assert!(display_str.contains("Doser("));
}
