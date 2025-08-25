use std::error::Error;
use std::sync::{Arc, Mutex};

use doser_core::{ControlCfg, Doser, FilterCfg, Timeouts};
use rstest::rstest;

// No custom clock needed for these tests

// Motor spy that records the last commanded speed
#[derive(Default, Clone)]
struct SpyMotor {
    pub last_sps: Arc<Mutex<u32>>,
}
impl doser_traits::Motor for SpyMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        *self.last_sps.lock().unwrap() = sps;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[rstest]
fn band_selection_matches_thresholds() {
    // Use default bands: [(1.0,1100),(0.5,450),(0.2,200)]
    // Use a calibration that provides 0.1 g/count resolution to avoid whole-gram rounding.
    // This ensures err_g matches the intended fractional values for band selection.
    let _doser = Doser::builder()
        .with_scale(doser_core::mocks::NoopScale)
        .with_motor(SpyMotor::default())
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg::default())
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 0.1,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(10.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap();

    // Helper: step once at current_g and capture sps via spy
    let check = |current_g: f32, expect_sps: u32| {
        let spy = SpyMotor::default();
        let spy_ref = spy.clone();
        let mut d = Doser::builder()
            .with_scale(doser_core::mocks::NoopScale)
            .with_motor(spy)
            .with_filter(FilterCfg {
                ma_window: 1,
                median_window: 1,
                sample_rate_hz: 50,
                ema_alpha: 0.0,
            })
            .with_control(ControlCfg::default())
            .with_timeouts(Timeouts { sensor_ms: 1 })
            .with_calibration(doser_core::Calibration {
                gain_g_per_count: 0.1,
                zero_counts: 0,
                offset_g: 0.0,
            })
            .with_target_grams(10.0)
            .apply_calibration::<()>(None)
            .build()
            .unwrap();
        // Convert grams to raw counts at 0.1 g/count resolution.
        let raw = (current_g * 10.0).round() as i32;
        let _ = d.step_from_raw(raw).unwrap();
        let sps = *spy_ref.last_sps.lock().unwrap();
        assert_eq!(sps, expect_sps, "current_g={current_g}");
    };

    check(8.8, 1100); // err_g=1.2
    check(9.3, 450); // err_g=0.7
    check(9.7, 200); // err_g=0.3
    check(9.9, 200); // err_g=0.1 -> lowest band (avoid rounding to 10.0)
}

// Simple sim types for integration test
#[derive(Default)]
struct SimState {
    weight_g: f32,
    sps: u32,
}

#[derive(Clone)]
// Use default clock

struct SimScale {
    st: Arc<Mutex<SimState>>,
    h: f32,
}
impl doser_traits::Scale for SimScale {
    fn read(&mut self, _timeout: std::time::Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        let mut st = self.st.lock().unwrap();
        st.weight_g += (st.sps as f32) * self.h / 50.0; // 50Hz step emulation
        Ok(st.weight_g.round() as i32)
    }
}
struct SimMotor {
    st: Arc<Mutex<SimState>>,
}
impl doser_traits::Motor for SimMotor {
    fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
    fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.st.lock().unwrap().sps = sps;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
        Ok(())
    }
}

#[rstest]
fn banded_vs_legacy_overshoot() {
    // Compare overshoot after completion with and without bands
    let st1 = Arc::new(Mutex::new(SimState::default()));
    let st2 = Arc::new(Mutex::new(SimState::default()));

    let mut doser_legacy = Doser::builder()
        .with_scale(SimScale {
            st: st1.clone(),
            h: 0.002,
        })
        .with_motor(SimMotor { st: st1.clone() })
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
        .with_target_grams(5.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap();

    let mut doser_band = Doser::builder()
        .with_scale(SimScale {
            st: st2.clone(),
            h: 0.002,
        })
        .with_motor(SimMotor { st: st2.clone() })
        .with_filter(FilterCfg {
            ma_window: 1,
            median_window: 1,
            sample_rate_hz: 50,
            ema_alpha: 0.0,
        })
        .with_control(ControlCfg {
            stable_ms: 0,
            ..ControlCfg::default()
        })
        .with_timeouts(Timeouts { sensor_ms: 1 })
        .with_calibration(doser_core::Calibration {
            gain_g_per_count: 1.0,
            zero_counts: 0,
            offset_g: 0.0,
        })
        .with_target_grams(5.0)
        .apply_calibration::<()>(None)
        .build()
        .unwrap();

    for _ in 0..500 {
        let _ = doser_legacy.step();
        if matches!(doser_legacy.step(), Ok(doser_core::DosingStatus::Complete)) {
            break;
        }
    }
    for _ in 0..500 {
        let _ = doser_band.step();
        if matches!(doser_band.step(), Ok(doser_core::DosingStatus::Complete)) {
            break;
        }
    }

    let o1 = (doser_legacy.last_weight() - 5.0).max(0.0);
    let o2 = (doser_band.last_weight() - 5.0).max(0.0);
    assert!(o2 <= o1 + 1e-3, "banded overshoot={o2} legacy={o1}");
}
