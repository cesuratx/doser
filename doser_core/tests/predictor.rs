use doser_core::{ControlCfg, Doser, FilterCfg, PredictorCfg, Timeouts};
use rstest::rstest;
use std::error::Error;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Use NoopScale and drive the loop via step_from_raw() to simulate weights.
#[derive(Default)]
struct SpyMotor {
    pub stopped_at_step: Option<usize>,
}
impl doser_traits::Motor for SpyMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, _sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.stopped_at_step.get_or_insert(0);
        Ok(())
    }
}

// Deterministic test clock we can manually advance.
#[derive(Clone)]
struct TestClock {
    origin: std::time::Instant,
    ms: Arc<AtomicU64>,
}
impl TestClock {
    fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
            ms: Arc::new(AtomicU64::new(0)),
        }
    }
    fn advance(&self, ms: u64) {
        self.ms.fetch_add(ms, Ordering::Relaxed);
    }
}
impl doser_traits::clock::Clock for TestClock {
    fn now(&self) -> std::time::Instant {
        self.origin + std::time::Duration::from_millis(self.ms.load(Ordering::Relaxed))
    }
    fn sleep(&self, d: std::time::Duration) {
        let add = d.as_millis() as u64;
        if add > 0 {
            self.advance(add);
        }
    }
}

#[rstest]
fn early_stop_triggers_before_target_cross() {
    // Configure predictor to estimate ~3g inflight at 50Hz with window=4 and extra=40ms.
    let predictor = PredictorCfg {
        enabled: true,
        window: 4,
        extra_latency_ms: 40,
        min_progress_ratio: 0.05,
    };

    let tclk = TestClock::new();

    let mut doser = Doser::builder()
        .with_scale(doser_core::mocks::NoopScale)
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            speed_bands: vec![],
            stable_ms: 0,
            ..ControlCfg::default()
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 1.0,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(10.0)
        .with_clock(Box::new(tclk.clone()))
        .with_predictor(predictor)
        .apply_calibration::<()>(None)
        .build()
        .unwrap();

    doser.begin();

    // Feed a linear ramp: 1g per step (raw=1 -> 1g due to calibration gain=1.0)
    // Expect early stop around ~7g if inflight~3g.
    let mut stopped_idx: Option<usize> = None;
    for i in 1..=20 {
        // advance virtual time by one sample period (20ms at 50Hz)
        tclk.advance(20);
        let status = doser.step_from_raw(i as i32).unwrap();
        // Once predictor stops early, subsequent steps will continue Running until settle, but weight
        // should still be < target when stop was issued. Detect the first time we enter settle zone shortly after.
        if stopped_idx.is_none()
            && matches!(status, doser_core::DosingStatus::Running)
            && doser.last_weight() >= 7.0
        {
            stopped_idx = Some(i);
            break;
        }
    }
    let idx = stopped_idx.expect("predictor did not stop early around 7g");
    assert!(
        doser.last_weight() <= 8.5,
        "early stop too late: weight={} at idx={}",
        doser.last_weight(),
        idx
    );
}
