//! Accuracy regression test under additive Gaussian noise and latency.
//!
//! Runs 20 trials at each target in {11, 15, 18, 20, 25} g with seeded
//! noise sigma in [0.02, 0.04] g. Asserts:
//! - P95(|final-true - target|) <= 0.3 g
//! - Max error <= 0.5 g
//! - No aborts (any AbortReason fails the test)

use doser_core::{ControlCfg, Doser, DosingStatus, FilterCfg, PredictorCfg, SafetyCfg, Timeouts};
use doser_traits::{Motor, Scale};
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
        // [0, 1)
        (self.next_u32() as f32) / (u32::MAX as f32 + 1.0)
    }
}

// Box-Muller transform for standard normal N(0,1)
#[derive(Clone)]
struct Gauss32 {
    rng: XorShift32,
    spare: Option<f32>,
}
impl Gauss32 {
    fn new(seed: u32) -> Self {
        Self {
            rng: XorShift32::new(seed),
            spare: None,
        }
    }
    fn next_std(&mut self) -> f32 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        // Avoid log(0)
        let u1 = (self.rng.next_f32()).clamp(f32::EPSILON, 1.0 - f32::EPSILON);
        let u2 = self.rng.next_f32();
        let r = (-2.0 * u1.ln()).sqrt();
        let th = 2.0 * core::f32::consts::PI * u2;
        let z0 = r * th.cos();
        let z1 = r * th.sin();
        self.spare = Some(z1);
        z0
    }
    fn next_with_sigma(&mut self, sigma: f32) -> f32 {
        self.next_std() * sigma
    }
}

#[derive(Default)]
struct SimState {
    weight_g: f32,
    sps: u32,
}

// Simulated motor: controls steps-per-second in shared state
struct SimMotor {
    st: Arc<Mutex<SimState>>,
}
impl Motor for SimMotor {
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

// Simulated scale with fixed latency and additive Gaussian noise on measurement
struct SimScaleGaussLatency {
    st: Arc<Mutex<SimState>>,
    // grams delivered per motor step
    h_g_per_step: f32,
    // additive measurement noise sigma (grams)
    noise_sigma_g: f32,
    // delay in samples at the configured sample rate
    delay_samples: usize,
    // conversion: grams to raw counts (e.g. 0.01 g/count)
    g_per_count: f32,
    // configured sample rate in Hz for per-tick mass integration
    sample_rate_hz: f32,
    // local gaussian RNG
    gauss: Gauss32,
    // latency buffer of raw counts
    buf: VecDeque<i32>,
}

impl SimScaleGaussLatency {
    #[allow(clippy::too_many_arguments)]
    fn new(
        st: Arc<Mutex<SimState>>,
        h_g_per_step: f32,
        noise_sigma_g: f32,
        delay_samples: usize,
        g_per_count: f32,
        sample_rate_hz: u32,
        seed: u32,
    ) -> Self {
        Self {
            st,
            h_g_per_step,
            noise_sigma_g,
            delay_samples,
            g_per_count,
            sample_rate_hz: sample_rate_hz as f32,
            gauss: Gauss32::new(seed),
            buf: VecDeque::with_capacity(delay_samples + 4),
        }
    }
}

impl Scale for SimScaleGaussLatency {
    fn read(&mut self, _timeout: std::time::Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
        // Advance plant one control tick worth of mass based on current sps
        let mut st = self.st.lock().unwrap();
        let delta_g = (st.sps as f32) * self.h_g_per_step / self.sample_rate_hz; // per tick
        st.weight_g += delta_g.max(0.0);

        // Produce a noisy measurement with fixed latency
        let meas_g = st.weight_g + self.gauss.next_with_sigma(self.noise_sigma_g);
        let raw_now = (meas_g / self.g_per_count).round() as i32;
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

fn percentile(values: &mut [f32], p: f32) -> f32 {
    assert!(!values.is_empty());
    values.sort_by(|a, b| a.total_cmp(b));
    let n = values.len();
    let rank = (p * (n as f32)).ceil().clamp(1.0, n as f32) as usize; // 1..=n
    values[rank - 1]
}

#[rstest]
fn accuracy_p95_and_max_under_noise() {
    const SAMPLE_RATE_HZ: u32 = 50;
    let tick_ms: u64 = (1000.0 / SAMPLE_RATE_HZ as f64).round() as u64;
    const DELAY_MS: u64 = 40; // scale latency per spec
    let delay_samples = ((DELAY_MS as f32) * (SAMPLE_RATE_HZ as f32) / 1000.0).round() as usize; // ~2
    const G_PER_COUNT: f32 = 0.01; // calibration: 0.01 g/count (1 cg)
    const K_G_PER_STEP: f32 = 0.0025; // grams per motor step

    // Control: three bands with slow approach; predictor enabled to compensate latency
    let control = ControlCfg {
        speed_bands: vec![(1.0, 1200), (0.6, 450), (0.2, 80)],
        stable_ms: 300,
        epsilon_g: 0.05,
        ..ControlCfg::default()
    };
    let predictor = PredictorCfg {
        enabled: true,
        window: 5,
        extra_latency_ms: DELAY_MS,
        min_progress_ratio: 0.1,
    };
    let filter = FilterCfg {
        ma_window: 1,
        median_window: 1,
        sample_rate_hz: SAMPLE_RATE_HZ,
        ema_alpha: 0.0,
    };
    let safety = SafetyCfg {
        max_run_ms: 60_000,
        max_overshoot_g: 2.0,
        no_progress_epsilon_g: 0.0,
        no_progress_ms: 0,
    };
    let timeouts = Timeouts { sensor_ms: 5 };

    let targets = [11.0_f32, 15.0, 18.0, 20.0, 25.0];
    const TRIALS: usize = 20;

    for &target in &targets {
        let mut errs: Vec<f32> = Vec::with_capacity(TRIALS);
        for trial in 0..TRIALS {
            // Sweep sigma from 0.02 to 0.04 deterministically across trials
            let sigma = 0.02_f32 + (0.02_f32 * (trial as f32) / (TRIALS as f32 - 1.0));

            let st = Arc::new(Mutex::new(SimState::default()));
            let scale = SimScaleGaussLatency::new(
                st.clone(),
                K_G_PER_STEP,
                sigma,
                delay_samples,
                G_PER_COUNT,
                SAMPLE_RATE_HZ,
                0xA11C_u32.wrapping_add((target as u32) * 31 + trial as u32),
            );
            let motor = SimMotor { st: st.clone() };
            let tclk = TestClock::new();

            let mut d = Doser::builder()
                .with_scale(scale)
                .with_motor(motor)
                .with_filter(filter.clone())
                .with_control(control.clone())
                .with_predictor(predictor.clone())
                .with_safety(safety.clone())
                .with_timeouts(timeouts.clone())
                .with_calibration(doser_core::Calibration {
                    gain_g_per_count: G_PER_COUNT,
                    zero_counts: 0,
                    offset_g: 0.0,
                })
                .with_target_grams(target)
                .with_clock(Box::new(tclk.clone()))
                .apply_calibration::<()>(None)
                .build()
                .unwrap();

            d.begin();

            let mut aborted = None;
            let mut completed = false;
            for _ in 0..2000 {
                tclk.advance(tick_ms);
                match d.step().unwrap() {
                    DosingStatus::Running => continue,
                    DosingStatus::Complete => {
                        completed = true;
                        break;
                    }
                    DosingStatus::Aborted(e) => {
                        aborted = Some(e);
                        break;
                    }
                }
            }
            if let Some(e) = aborted {
                panic!("unexpected abort at {target}g, trial {trial}: {e}");
            }
            assert!(completed, "did not complete at {target}g, trial {trial}");

            let true_final = st.lock().unwrap().weight_g;
            let err = (true_final - target).abs();
            errs.push(err);
        }

        // Compute P95 and max
        let mut errs_sorted = errs.clone();
        let p95 = percentile(&mut errs_sorted, 0.95);
        let max_err = errs_sorted.last().copied().unwrap_or(0.0);

        assert!(
            p95 <= 0.3,
            "P95 error too high at target {target}g: p95={p95:.3}g, errs={errs:?}"
        );
        assert!(
            max_err <= 0.5,
            "Max error too high at target {target}g: max={max_err:.3}g, errs={errs:?}"
        );
    }
}
