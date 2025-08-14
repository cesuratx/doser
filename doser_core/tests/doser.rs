use doser_core::*;
use std::sync::{Arc, Mutex};

struct MockScale {
    weights: Arc<Mutex<Vec<f32>>>,
}
impl doser_hardware::Scale for MockScale {
    fn read_weight(&mut self) -> f32 {
        let mut w = self.weights.lock().unwrap();
        w.remove(0)
    }
    fn tare(&mut self) {}
    fn calibrate(&mut self, _known_weight: f32) {}
}

struct MockMotor {
    started: Arc<Mutex<bool>>,
    stopped: Arc<Mutex<bool>>,
}
impl doser_hardware::Motor for MockMotor {
    fn start(&mut self) {
        *self.started.lock().unwrap() = true;
    }
    fn stop(&mut self) {
        *self.stopped.lock().unwrap() = true;
    }
}

#[test]
fn test_doser_stepwise_dosing_complete() {
    let weights = Arc::new(Mutex::new(vec![1.0, 2.0, 3.0, 5.0]));
    let scale = Box::new(MockScale {
        weights: weights.clone(),
    });
    let motor = Box::new(MockMotor {
        started: Arc::new(Mutex::new(false)),
        stopped: Arc::new(Mutex::new(false)),
    });
    let mut doser = Doser::new(scale, motor, 4.0, 2);
    let mut status = doser.step().unwrap();
    assert_eq!(status, DosingStatus::Running);
    status = doser.step().unwrap();
    assert_eq!(status, DosingStatus::Running);
    status = doser.step().unwrap();
    assert_eq!(status, DosingStatus::Running);
    status = doser.step().unwrap();
    assert_eq!(status, DosingStatus::Complete);
}

#[test]
fn test_doser_filtered_weight() {
    let weights = Arc::new(Mutex::new(vec![2.0, 4.0, 6.0]));
    let scale = Box::new(MockScale {
        weights: weights.clone(),
    });
    let motor = Box::new(MockMotor {
        started: Arc::new(Mutex::new(false)),
        stopped: Arc::new(Mutex::new(false)),
    });
    let mut doser = Doser::new(scale, motor, 10.0, 2);
    doser.step().unwrap();
    doser.step().unwrap();
    assert!((doser.filtered_weight() - 3.0).abs() < 1e-6);
}

#[test]
fn test_doser_debug_display() {
    let weights = Arc::new(Mutex::new(vec![1.0, 2.0]));
    let scale = Box::new(MockScale {
        weights: weights.clone(),
    });
    let motor = Box::new(MockMotor {
        started: Arc::new(Mutex::new(false)),
        stopped: Arc::new(Mutex::new(false)),
    });
    let doser = Doser::new(scale, motor, 2.0, 2);
    let debug_str = format!("{:?}", doser);
    let display_str = format!("{}", doser);
    assert!(debug_str.contains("Doser"));
    assert!(display_str.contains("Doser("));
}
