use doser_core::{ControlCfg, Doser, FilterCfg, PredictorCfg, SafetyCfg, Timeouts};
use rstest::rstest;
use std::collections::VecDeque;
use std::error::Error;
use std::sync::{Arc, Mutex};

// Deterministic tiny PRNG (xorshift32)
#[derive(Clone)]
struct XorShift32 {
    state: u32,
}
impl XorShift32 {
    fn new(seed: u32) -> Self {
        Self { state: seed.max(1) }
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }
    fn next_f32(&mut self) -> f32 {
        // [0,1)
        (self.next_u32() as f32) / (u32::MAX as f32 + 1.0)
    }
    fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
}

#[derive(Default)]
struct SimState {
    weight_g: f32,
    sps: u32,
}

// Simulated motor: sets steps-per-second in shared state
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
        self.st.lock().unwrap().sps = 0;
        Ok(())
    }
}

// Simulated scale with fixed latency and multiplicative noise on delivered mass
struct SimScaleLatency {
    st: Arc<Mutex<SimState>>,
    // grams delivered per motor step
    h_g_per_step: f32,
    // uniform noise amplitude (e.g. 0.03 => +/-3%)
    noise_amp: f32,
    // delay in samples at the configured sample rate
    delay_samples: usize,
    // conversion: grams to raw counts (e.g. 0.1 g/count)
    g_per_count: f32,
    // configured sample rate in Hz for per-tick mass integration
    sample_rate_hz: f32,
    // local PRNG
    rng: XorShift32,
    // latency buffer of raw counts
    buf: VecDeque<i32>,
}

impl SimScaleLatency {
    fn new(
        st: Arc<Mutex<SimState>>,
        h_g_per_step: f32,
        noise_amp: f32,
        delay_samples: usize,
        g_per_count: f32,
        sample_rate_hz: u32,
        seed: u32,
    ) -> Self {
        Self {
            st,
            h_g_per_step,
            noise_amp,
            delay_samples,
            g_per_count,
            sample_rate_hz: sample_rate_hz as f32,
            rng: XorShift32::new(seed),
            buf: VecDeque::with_capacity(delay_samples + 4),
        }
    }
}

impl doser_traits::Scale for SimScaleLatency {
    fn read(&mut self, _timeout: std::time::Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        // Advance plant one control tick worth of mass based on current sps and noise
        // Use configured sample rate instead of assuming 50 Hz
        let mut st = self.st.lock().unwrap();
        let noise = self
            .rng
            .next_range(1.0 - self.noise_amp, 1.0 + self.noise_amp);
        let delta_g = (st.sps as f32) * self.h_g_per_step * noise / self.sample_rate_hz; // per tick
        st.weight_g += delta_g.max(0.0);

        // Convert true grams to raw counts and push into latency buffer
        let raw_now = (st.weight_g / self.g_per_count).round() as i32;
        self.buf.push_back(raw_now);

        // Emit delayed sample if available; else 0 at startup
        let out = if self.buf.len() > self.delay_samples {
            self.buf.pop_front().unwrap_or(0)
        } else {
            0
        };
        Ok(out)
    }
}

// Deterministic test clock to advance virtual time
#[derive(Clone)]
struct TestClock {
    origin: std::time::Instant,
    ms: Arc<std::sync::atomic::AtomicU64>,
}
impl TestClock {
    fn new() -> Self {
        use std::sync::atomic::AtomicU64;
        Self {
            origin: std::time::Instant::now(),
            ms: Arc::new(AtomicU64::new(0)),
        }
    }
    fn advance(&self, ms: u64) {
        self.ms.fetch_add(ms, std::sync::atomic::Ordering::Relaxed);
    }
}
impl doser_traits::clock::Clock for TestClock {
    fn now(&self) -> std::time::Instant {
        self.origin
            + std::time::Duration::from_millis(self.ms.load(std::sync::atomic::Ordering::Relaxed))
    }
    fn sleep(&self, d: std::time::Duration) {
        let add = d.as_millis() as u64;
        if add > 0 {
            self.advance(add);
        }
    }
}

#[rstest]
fn predictor_reduces_overshoot_and_failures_under_latency() {
    const SAMPLE_RATE_HZ: u32 = 50;
    let tick_ms: u64 = (1000.0 / SAMPLE_RATE_HZ as f64).round() as u64;
    const DELAY_MS: u64 = 40; // scale latency per spec
    let delay_samples = ((DELAY_MS as f32) * (SAMPLE_RATE_HZ as f32) / 1000.0).round() as usize; // ~2
    const G_PER_COUNT: f32 = 0.01; // calibration: 0.01 g/count (1 cg)
    const K_G_PER_STEP: f32 = 0.0025; // grams per motor step
    const NOISE_AMP: f32 = 0.02; // +/-2%
    const TARGET_G: f32 = 5.0;
    const TRIALS: usize = 20;

    // Control band constants for readability
    const AGGR_BAND1_THR_G: f32 = 1.0;
    const AGGR_COARSE_SPS: u32 = 1200;
    const AGGR_BAND2_THR_G: f32 = 0.2;
    const AGGR_FINE_SPS: u32 = 600;

    const PRED_BAND1_THR_G: f32 = 1.0;
    const PRED_COARSE_SPS: u32 = 1200;
    const PRED_BAND2_THR_G: f32 = 0.6;
    const PRED_MEDIUM_SPS: u32 = 450;
    const PRED_BAND3_THR_G: f32 = 0.2;
    const PRED_FINE_SPS: u32 = 80;

    // Common configs (values used each iteration; instantiated per trial)

    // Case A: predictor OFF, 2-band speeds
    let control_a = ControlCfg {
        // Two-band control: aggressive fine speed to induce overshoot
        speed_bands: vec![
            (AGGR_BAND1_THR_G, AGGR_COARSE_SPS),
            (AGGR_BAND2_THR_G, AGGR_FINE_SPS),
        ],
        stable_ms: 0,
        epsilon_g: 0.0,
        ..ControlCfg::default()
    };
    // Case B: predictor ON, 3-band speeds
    let control_b = ControlCfg {
        // Three-band control: much slower near target
        speed_bands: vec![
            (PRED_BAND1_THR_G, PRED_COARSE_SPS),
            (PRED_BAND2_THR_G, PRED_MEDIUM_SPS),
            (PRED_BAND3_THR_G, PRED_FINE_SPS),
        ],
        stable_ms: 0,
        epsilon_g: 0.0,
        ..ControlCfg::default()
    };
    // Predictor settings for case B

    // Accumulators
    let mut sum_over_a = 0.0f32;
    let mut sum_over_b = 0.0f32;
    let mut n_done_a = 0usize;
    let mut n_done_b = 0usize;
    let mut aborts_a = 0usize;
    let mut aborts_b = 0usize;

    for trial in 0..TRIALS {
        // A
        {
            let st = Arc::new(Mutex::new(SimState::default()));
            let scale = SimScaleLatency::new(
                st.clone(),
                K_G_PER_STEP,
                NOISE_AMP,
                delay_samples,
                G_PER_COUNT,
                SAMPLE_RATE_HZ,
                0xACE1 + trial as u32,
            );
            let motor = SimMotor { st: st.clone() };
            let tclk = TestClock::new();
            let base_filter = FilterCfg {
                ma_window: 1,
                median_window: 1,
                sample_rate_hz: SAMPLE_RATE_HZ,
                ema_alpha: 0.0,
            };
            let safety = SafetyCfg {
                max_run_ms: 60_000,
                max_overshoot_g: 0.01, // 1 cg threshold
                no_progress_epsilon_g: 0.0,
                no_progress_ms: 0,
            };
            let timeouts = Timeouts { sensor_ms: 5 };
            let mut d = Doser::builder()
                .with_scale(scale)
                .with_motor(motor)
                .with_filter(base_filter)
                .with_control(control_a.clone())
                .with_safety(safety)
                .with_timeouts(timeouts)
                .with_calibration(doser_core::Calibration {
                    gain_g_per_count: G_PER_COUNT,
                    zero_counts: 0,
                    offset_g: 0.0,
                })
                .with_target_grams(TARGET_G)
                .with_clock(Box::new(tclk.clone()))
                .apply_calibration::<()>(None)
                .build()
                .unwrap();
            d.begin();
            let mut overshoot_abort = false;
            for _ in 0..1000 {
                tclk.advance(tick_ms);
                match d.step().unwrap() {
                    doser_core::DosingStatus::Aborted(e) => {
                        if format!("{e}").to_lowercase().contains("overshoot") {
                            overshoot_abort = true;
                        }
                        break;
                    }
                    doser_core::DosingStatus::Complete => break,
                    _ => {}
                }
            }
            if overshoot_abort {
                aborts_a += 1;
            }
            // Measure true plant overshoot
            let over_true = (st.lock().unwrap().weight_g - TARGET_G).max(0.0);
            if !overshoot_abort {
                sum_over_a += over_true;
                n_done_a += 1;
            }
        }

        // B
        {
            let st = Arc::new(Mutex::new(SimState::default()));
            let scale = SimScaleLatency::new(
                st.clone(),
                K_G_PER_STEP,
                NOISE_AMP,
                delay_samples,
                G_PER_COUNT,
                SAMPLE_RATE_HZ,
                0xBEEF + trial as u32,
            );
            let motor = SimMotor { st: st.clone() };
            let tclk = TestClock::new();
            let base_filter = FilterCfg {
                ma_window: 1,
                median_window: 1,
                sample_rate_hz: SAMPLE_RATE_HZ,
                ema_alpha: 0.0,
            };
            let safety = SafetyCfg {
                max_run_ms: 60_000,
                max_overshoot_g: 0.01, // 1 cg threshold
                no_progress_epsilon_g: 0.0,
                no_progress_ms: 0,
            };
            let timeouts = Timeouts { sensor_ms: 5 };
            let predictor = PredictorCfg {
                enabled: true,
                window: 5,
                extra_latency_ms: DELAY_MS as u64,
                min_progress_ratio: 0.1,
            };
            let mut d = Doser::builder()
                .with_scale(scale)
                .with_motor(motor)
                .with_filter(base_filter)
                .with_control(control_b.clone())
                .with_predictor(predictor)
                .with_safety(safety)
                .with_timeouts(timeouts)
                .with_calibration(doser_core::Calibration {
                    gain_g_per_count: G_PER_COUNT,
                    zero_counts: 0,
                    offset_g: 0.0,
                })
                .with_target_grams(TARGET_G)
                .with_clock(Box::new(tclk.clone()))
                .apply_calibration::<()>(None)
                .build()
                .unwrap();
            d.begin();
            let mut overshoot_abort = false;
            for _ in 0..1000 {
                tclk.advance(tick_ms);
                match d.step().unwrap() {
                    doser_core::DosingStatus::Aborted(e) => {
                        if format!("{e}").to_lowercase().contains("overshoot") {
                            overshoot_abort = true;
                        }
                        break;
                    }
                    doser_core::DosingStatus::Complete => break,
                    _ => {}
                }
            }
            if overshoot_abort {
                aborts_b += 1;
            }
            let over_true = (st.lock().unwrap().weight_g - TARGET_G).max(0.0);
            if !overshoot_abort {
                sum_over_b += over_true;
                n_done_b += 1;
            }
        }
    }

    // Ensure we have completions to compute means
    assert!(
        n_done_a > 0 && n_done_b > 0,
        "insufficient completed trials: A={n_done_a}, B={n_done_b}"
    );
    let mean_a = sum_over_a / n_done_a as f32;
    let mean_b = sum_over_b / n_done_b as f32;
    assert!(
        mean_b <= 0.6 * mean_a,
        "mean overshoot did not drop enough: A={mean_a:.3}g, B={mean_b:.3}g"
    );
    assert!(
        aborts_b < aborts_a,
        "overshoot aborts did not decrease: A={aborts_a}, B={aborts_b}"
    );
}
