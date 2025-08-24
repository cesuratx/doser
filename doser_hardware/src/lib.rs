//! doser_hardware: hardware and simulation backends behind `doser_traits`.
//!
//! Features:
//! - `hardware`: enable Raspberry Pi GPIO/HX711-backed implementations.
//! - (default) no `hardware` feature: use simulation types that satisfy the traits.
//!
//! Note: The `rppal` dependency is optional and only enabled when the `hardware`
//!       feature is active. This lets CI on x86 build without pulling GPIO libs.

pub mod error;
pub mod util;

// Make the HX711 driver module available when hardware feature is enabled on Linux.
#[cfg(all(feature = "hardware", target_os = "linux"))]
mod hx711;

// Provide the simulation backend when hardware is disabled OR when not on Linux.
// This ensures cross-platform builds work even if the `hardware` feature is toggled on.
#[cfg(any(not(feature = "hardware"), not(target_os = "linux")))]
pub mod sim {
    use doser_traits::{Motor, Scale};
    use std::error::Error;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Duration;

    static SIM_RUNNING: AtomicBool = AtomicBool::new(false);
    static SIM_SPS: AtomicU32 = AtomicU32::new(0);

    /// Minimal simulated scale that increments by an optional env-configured delta while running.
    pub struct SimulatedScale {
        grams: f32,
    }

    impl Default for SimulatedScale {
        fn default() -> Self {
            Self::new()
        }
    }

    impl SimulatedScale {
        pub fn new() -> Self {
            Self { grams: 0.0 }
        }
    }

    impl Scale for SimulatedScale {
        fn read(&mut self, _timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            // Optional: simulate a blocking timeout when DOSER_TEST_SIM_TIMEOUT is set.
            if std::env::var("DOSER_TEST_SIM_TIMEOUT")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
                > 0
            {
                // Sleep roughly the requested timeout to mimic a real blocking call.
                // The controller's watchdog logic does not depend on the exact sleep here.
                // Use a small upper bound to avoid long stalls in tests.
                let sleep_for = _timeout.min(Duration::from_millis(10));
                std::thread::sleep(sleep_for);
                let err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
                return Err(Box::new(err));
            }
            let delta = std::env::var("DOSER_TEST_SIM_INC")
                .ok()
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(0.0);
            if SIM_RUNNING.load(Ordering::Relaxed) && delta != 0.0 {
                let _sps = SIM_SPS.load(Ordering::Relaxed);
                // Keep it simple for now: one delta per read while running
                self.grams = (self.grams + delta).max(0.0);
            }
            // For the sim, return raw counts with 0.01 g resolution (centigrams)
            Ok((self.grams * 100.0) as i32)
        }
    }

    /// Minimal simulated motor; tracks speed and running state.
    #[derive(Default)]
    pub struct SimulatedMotor {
        speed_sps: u32,
        running: bool,
    }

    impl Motor for SimulatedMotor {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running = true;
            SIM_RUNNING.store(true, Ordering::Relaxed);
            Ok(())
        }

        fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
            // You can enforce "must start first" semantics if you want:
            // if !self.running { return Err("motor not started".into()); }
            self.speed_sps = sps;
            SIM_SPS.store(sps, Ordering::Relaxed);
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.speed_sps = 0;
            self.running = false;
            SIM_SPS.store(0, Ordering::Relaxed);
            SIM_RUNNING.store(false, Ordering::Relaxed);
            Ok(())
        }
    }
}

// Generic absolute-deadline pacer with pluggable sleeper for testability
// When not building Linux+hardware, these items may be unused; silence warnings.
#[cfg_attr(not(all(feature = "hardware", target_os = "linux")), allow(dead_code))]
mod pacing {
    use std::time::{Duration, Instant};

    #[allow(dead_code)]
    pub trait Sleeper {
        fn now(&self) -> Instant;
        fn sleep_until(&self, deadline: Instant);
    }

    pub struct RealSleeper;
    impl Sleeper for RealSleeper {
        fn now(&self) -> Instant {
            Instant::now()
        }
        fn sleep_until(&self, deadline: Instant) {
            let now = Instant::now();
            if deadline <= now {
                return;
            }
            let delta = deadline - now;
            #[cfg(all(feature = "rt", target_os = "linux"))]
            {
                use libc::{
                    CLOCK_MONOTONIC, TIMER_ABSTIME, clock_gettime, clock_nanosleep, timespec,
                };
                unsafe {
                    let mut now_ts = timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    };
                    if clock_gettime(CLOCK_MONOTONIC, &mut now_ts) == 0 {
                        let (sec, nsec) =
                            add_duration_to_timespec(now_ts.tv_sec, now_ts.tv_nsec, delta);
                        let target = timespec {
                            tv_sec: sec,
                            tv_nsec: nsec,
                        };
                        loop {
                            let rc = clock_nanosleep(
                                CLOCK_MONOTONIC,
                                TIMER_ABSTIME,
                                &target,
                                std::ptr::null_mut(),
                            );
                            if rc == 0 {
                                return;
                            }
                            if rc != libc::EINTR {
                                break;
                            }
                        }
                    }
                }
            }
            std::thread::sleep(delta);
        }
    }

    /// Absolute-deadline pacer; measures jitter and exposes rolling average.
    pub struct Pacer {
        next_deadline: Instant,
        initialized: bool,
        jitter_accum_us: u128,
        jitter_count: u32,
        pub avg_jitter_us: u32,
    }

    impl Pacer {
        pub fn new() -> Self {
            Self {
                next_deadline: Instant::now(),
                initialized: false,
                jitter_accum_us: 0,
                jitter_count: 0,
                avg_jitter_us: 0,
            }
        }
        pub fn reset(&mut self) {
            self.initialized = false;
        }

        /// Run one full period paced by absolute deadlines, invoking `mid_hook` exactly at half period.
        /// Returns Some(avg_jitter_us) whenever the internal window (256) completes.
        pub fn step_with<F, S: Sleeper>(
            &mut self,
            sleeper: &S,
            period_us: u64,
            mut mid_hook: F,
        ) -> Option<u32>
        where
            F: FnMut(),
        {
            let period = Duration::from_micros(period_us.max(1));
            let half = period / 2;
            if !self.initialized {
                self.next_deadline = sleeper.now() + period;
                self.initialized = true;
            }
            let mid = self.next_deadline - half;
            sleeper.sleep_until(mid);
            mid_hook();
            sleeper.sleep_until(self.next_deadline);

            let now = sleeper.now();
            let jitter = if now >= self.next_deadline {
                now - self.next_deadline
            } else {
                self.next_deadline - now
            };
            self.jitter_accum_us = self.jitter_accum_us.saturating_add(jitter.as_micros());
            self.jitter_count = self.jitter_count.saturating_add(1);
            self.next_deadline += period;
            if self.jitter_count >= 256 {
                let avg = (self.jitter_accum_us / (self.jitter_count as u128)) as u32;
                self.avg_jitter_us = avg;
                self.jitter_accum_us = 0;
                self.jitter_count = 0;
                Some(avg)
            } else {
                None
            }
        }

        pub fn step<S: Sleeper>(&mut self, sleeper: &S, period_us: u64) -> Option<u32> {
            self.step_with(sleeper, period_us, || {})
        }
    }

    /// Add a Duration to a timespec-like (sec, nsec) pair, normalizing nanoseconds and saturating seconds.
    #[inline]
    fn add_duration_to_timespec(now_sec: i64, now_nsec: i64, delta: Duration) -> (i64, i64) {
        let add_sec_i64 = i64::try_from(delta.as_secs()).unwrap_or(i64::MAX);
        let add_nsec_i64 = delta.subsec_nanos() as i64; // < 1e9
        let mut sec = now_sec.saturating_add(add_sec_i64);
        let mut nsec = now_nsec.saturating_add(add_nsec_i64);
        if nsec >= 1_000_000_000 {
            let carry = nsec / 1_000_000_000;
            sec = sec.saturating_add(carry);
            nsec -= carry * 1_000_000_000;
        } else if nsec < 0 {
            let borrow = 1 + ((-nsec) / 1_000_000_000);
            sec = sec.saturating_sub(borrow);
            nsec += borrow * 1_000_000_000;
        }
        (sec, nsec)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        // Test-only fake sleeper that advances a virtual clock.
        pub struct FakeSleeper {
            origin: Instant,
            offset: std::sync::Arc<std::sync::Mutex<Duration>>,
        }
        impl FakeSleeper {
            pub fn new() -> Self {
                Self {
                    origin: Instant::now(),
                    offset: std::sync::Arc::new(std::sync::Mutex::new(Duration::ZERO)),
                }
            }
            pub fn elapsed(&self) -> Duration {
                *self.offset.lock().unwrap()
            }
        }
        impl Sleeper for FakeSleeper {
            fn now(&self) -> Instant {
                self.origin + *self.offset.lock().unwrap()
            }
            fn sleep_until(&self, deadline: Instant) {
                let mut off = self.offset.lock().unwrap();
                let dur = deadline.saturating_duration_since(self.origin);
                *off = dur;
            }
        }

        #[test]
        fn add_no_carry() {
            let (s, ns) =
                add_duration_to_timespec(10, 500_000_000, Duration::from_nanos(400_000_000));
            assert_eq!((s, ns), (10, 900_000_000));
        }
        #[test]
        fn add_single_carry() {
            let (s, ns) =
                add_duration_to_timespec(10, 700_000_000, Duration::from_nanos(500_000_000));
            assert_eq!((s, ns), (11, 200_000_000));
        }
        #[test]
        fn add_multi_second_carry() {
            let (s, ns) = add_duration_to_timespec(10, 900_000_000, Duration::new(3, 200_000_000));
            assert_eq!((s, ns), (14, 100_000_000));
        }
        #[test]
        fn saturate_on_large_secs() {
            let (s, ns) =
                add_duration_to_timespec(i64::MAX - 5, 0, Duration::new(u64::MAX, 999_999_999));
            assert_eq!(s, i64::MAX);
            assert!(ns < 1_000_000_000);
        }

        #[test]
        fn no_drift_after_many_cycles_with_fake_sleep() {
            let mut pacer = Pacer::new();
            let sleeper = FakeSleeper::new();
            let period_us = 1000u64;
            for _ in 0..10_000u32 {
                let _ = pacer.step(&sleeper, period_us);
            }
            let expected = Duration::from_micros(period_us * 10_000);
            assert_eq!(sleeper.elapsed(), expected);
        }
    }
}

#[cfg(all(feature = "hardware", target_os = "linux"))]
pub mod hardware {
    use crate::error::{HwError, Result as HwResult};
    use crate::hx711::Hx711;
    use crate::pacing::{Pacer, RealSleeper};
    use doser_traits::clock::{Clock, MonotonicClock};
    use doser_traits::{Motor, Scale};
    use rppal::gpio::{Gpio, OutputPin};
    use std::error::Error;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    };
    use std::thread::{self, JoinHandle};
    use std::time::Duration;
    use tracing::{info, warn};

    /// Hardware scale backed by HX711.
    pub struct HardwareScale {
        hx: Hx711,
    }

    impl HardwareScale {
        /// Create a new HX711-backed scale using DT and SCK GPIO pins.
        pub fn try_new(dt_pin: u8, sck_pin: u8) -> HwResult<Self> {
            let gpio =
                Gpio::new().map_err(|e| HwError::Gpio(format!("open GPIO for HX711: {e}")))?;
            let dt = gpio
                .get(dt_pin)
                .map_err(|e| HwError::Gpio(format!("get HX711 DT pin: {e}")))?
                .into_input();
            let sck = gpio
                .get(sck_pin)
                .map_err(|e| HwError::Gpio(format!("get HX711 SCK pin: {e}")))?
                .into_output_low();
            // Channel A, gain = 128 uses 25 pulses after the 24-bit read.
            let hx = Hx711::new(dt, sck, 25, Duration::from_millis(150))?;
            Ok(Self { hx })
        }

        /// Create HX711-backed scale with explicit data-ready timeout (ms).
        pub fn try_new_with_timeout(
            dt_pin: u8,
            sck_pin: u8,
            data_ready_timeout_ms: u64,
        ) -> HwResult<Self> {
            let gpio =
                Gpio::new().map_err(|e| HwError::Gpio(format!("open GPIO for HX711: {e}")))?;
            let dt = gpio
                .get(dt_pin)
                .map_err(|e| HwError::Gpio(format!("get HX711 DT pin: {e}")))?
                .into_input();
            let sck = gpio
                .get(sck_pin)
                .map_err(|e| HwError::Gpio(format!("get HX711 SCK pin: {e}")))?
                .into_output_low();
            let drt = if data_ready_timeout_ms == 0 {
                150
            } else {
                data_ready_timeout_ms
            };
            let hx = Hx711::new(dt, sck, 25, Duration::from_millis(drt))?;
            Ok(Self { hx })
        }

        /// Read a raw 24-bit value from HX711 with timeout.
        fn read_raw_timeout(
            &mut self,
            timeout: Duration,
        ) -> Result<i32, Box<dyn Error + Send + Sync>> {
            self.hx
                .read_with_timeout(timeout)
                .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(e) })
        }
    }

    impl Scale for HardwareScale {
        fn read(&mut self, timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>> {
            self.read_raw_timeout(timeout)
        }
    }

    /// Raspberry Pi step/dir motor driver with optional enable pin.
    pub struct HardwareMotor {
        dir: OutputPin,
        en: Option<OutputPin>,
        running: Arc<AtomicBool>,
        sps: Arc<AtomicU32>,
        handle: Option<JoinHandle<()>>,
        shutdown_tx: mpsc::Sender<()>,
        // Expose rough jitter stat (average over last window) for observability
        avg_jitter_us: Arc<AtomicU32>,
    }

    impl HardwareMotor {
        /// Create a motor from GPIO pin numbers. EN is taken from the DOSER_EN_PIN env var if present.
        pub fn try_new(step_pin: u8, dir_pin: u8) -> HwResult<Self> {
            let en_env = std::env::var("DOSER_EN_PIN")
                .ok()
                .and_then(|s| s.parse::<u8>().ok());
            Self::try_new_with_en(step_pin, dir_pin, en_env)
        }

        /// Create a motor from GPIO pin numbers with an optional enable pin.
        /// Note: On A4988/DRV8825, EN is active-low (low = enabled). We default to disabled (high).
        pub fn try_new_with_en(step_pin: u8, dir_pin: u8, en_pin: Option<u8>) -> HwResult<Self> {
            let gpio = Gpio::new().map_err(|e| HwError::Gpio(format!("open GPIO: {e}")))?;
            let mut step = gpio
                .get(step_pin)
                .map_err(|e| HwError::Gpio(format!("get STEP pin: {e}")))?
                .into_output_low();
            let dir = gpio
                .get(dir_pin)
                .map_err(|e| HwError::Gpio(format!("get DIR pin: {e}")))?
                .into_output_low();

            let en = match en_pin {
                Some(pin) => Some(
                    gpio.get(pin)
                        .map_err(|e| HwError::Gpio(format!("get EN pin: {e}")))?
                        .into_output_high(),
                ), // high = disabled
                None => None,
            };

            let running = Arc::new(AtomicBool::new(false));
            let sps = Arc::new(AtomicU32::new(0));
            let (shutdown_tx, shutdown_rx): (mpsc::Sender<()>, mpsc::Receiver<()>) =
                mpsc::channel();

            let running_bg = running.clone();
            let sps_bg = sps.clone();
            let avg_jitter_us = Arc::new(AtomicU32::new(0));
            let avg_jitter_us_bg = avg_jitter_us.clone();
            // Move STEP into the background thread; not used elsewhere.
            let handle = thread::spawn(move || {
                let clock = MonotonicClock::new();
                // Optional: try to elevate RT priority and lock memory when feature is enabled
                #[cfg(feature = "rt")]
                if let Err(e) = setup_realtime() {
                    tracing::warn!(error = %e, "rt setup failed; continuing non-RT");
                }

                let mut pacer = Pacer::new();
                let sleeper = RealSleeper;

                loop {
                    if shutdown_rx.try_recv().is_ok() {
                        break;
                    }

                    let is_running = running_bg.load(Ordering::Relaxed);
                    let sps_val = sps_bg.load(Ordering::Relaxed).clamp(0, 5_000);
                    if !(is_running && sps_val > 0) {
                        clock.sleep(Duration::from_millis(2));
                        pacer.reset();
                        continue;
                    }

                    let period_us = (1_000_000u32 / sps_val).max(1) as u64; // us
                    // Rising edge
                    let _ = step.set_high();
                    spin_delay_min();
                    busy_wait_min_1us();
                    // High hold until mid, then fall and hold until end
                    if let Some(avg) = pacer.step_with(&sleeper, period_us, || {
                        let _ = step.set_low();
                        spin_delay_min();
                        busy_wait_min_1us();
                    }) {
                        avg_jitter_us_bg.store(avg, Ordering::Relaxed);
                    }
                }
            });

            let mut motor = Self {
                dir,
                en,
                running,
                sps,
                handle: Some(handle),
                shutdown_tx,
                avg_jitter_us,
            };
            // Default: disabled
            let _ = motor.set_enabled(false);
            Ok(motor)
        }

        /// Set direction: true = clockwise (DIR high), false = counterclockwise (DIR low)
        pub fn set_direction(&mut self, clockwise: bool) {
            if clockwise {
                let _ = self.dir.set_high();
            } else {
                let _ = self.dir.set_low();
            }
        }

        /// Enable or disable the driver (active-low enable pin, if present)
        pub fn set_enabled(&mut self, enabled: bool) -> HwResult<()> {
            if let Some(en) = self.en.as_mut() {
                if enabled {
                    en.set_low();
                } else {
                    en.set_high();
                }
            }
            Ok(())
        }

        /// Set speed in steps-per-second; worker thread reads this atomically.
        pub fn set_speed_sps(&mut self, sps: u32) {
            self.sps.store(sps, Ordering::Relaxed);
        }
    }

    impl Drop for HardwareMotor {
        fn drop(&mut self) {
            let _ = self.shutdown_tx.send(());
            self.running.store(false, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
            // Disable on drop
            let _ = self.set_enabled(false);
        }
    }

    impl Motor for HardwareMotor {
        fn start(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.set_enabled(true)
                .map_err(|e| Box::<dyn Error + Send + Sync>::from(e))?;
            self.running.store(true, Ordering::Relaxed);
            info!("motor started");
            Ok(())
        }

        fn set_speed(&mut self, sps: u32) -> Result<(), Box<dyn Error + Send + Sync>> {
            let clamped = sps.clamp(0, 5_000);
            if clamped == 0 {
                warn!("requested 0 sps; motor will idle");
            }
            self.set_speed_sps(clamped);
            Ok(())
        }

        fn stop(&mut self) -> Result<(), Box<dyn Error + Send + Sync>> {
            self.running.store(false, Ordering::Relaxed);
            self.set_speed_sps(0);
            info!("motor stopped");
            Ok(())
        }
    }

    /// Return average jitter in microseconds over the last window (approximate).
    impl HardwareMotor {
        pub fn avg_jitter_us(&self) -> u32 {
            self.avg_jitter_us.load(Ordering::Relaxed)
        }
    }

    /// Very small spin to make edges clean.
    #[inline(always)]
    fn spin_delay_min() {
        std::hint::spin_loop();
    }

    /// Sleep for microseconds using std; coarse but sufficient for <= 5 kHz.
    fn spin_sleep_us(us: u64) {
        MonotonicClock::new().sleep(Duration::from_micros(us));
    }

    /// Busy-wait for at least ~1 microsecond to cleanly separate edges.
    #[inline(always)]
    fn busy_wait_min_1us() {
        // Calibrate a rough spin count for ~1µs once per process, then spin that many times.
        // This avoids relying on a fixed number of spin_loop() calls which varies by CPU.
        #[inline]
        fn spins_per_us() -> u32 {
            use std::sync::OnceLock;
            static SPINS: OnceLock<u32> = OnceLock::new();
            *SPINS.get_or_init(|| {
                use std::time::{Duration, Instant};
                // Try increasing iteration counts until we measure ≥ 100µs to reduce timer noise.
                let mut iters: u32 = 1_000;
                let mut per_us: u32 = 2; // conservative fallback
                for _ in 0..10 {
                    let start = Instant::now();
                    // Volatile loop to prevent unrolling/elimination
                    let mut i = 0u32;
                    while i < iters {
                        std::hint::spin_loop();
                        i = i.wrapping_add(1);
                    }
                    let dt = start.elapsed();
                    if dt >= Duration::from_micros(100) {
                        // per_us ≈ iters / elapsed_us
                        let us = dt.as_micros().max(1) as u64;
                        per_us = ((iters as u64 + us - 1) / us).clamp(1, 1_000_000) as u32;
                        break;
                    }
                    // Increase work and try again
                    iters = iters.saturating_mul(4).min(4_000_000);
                }
                per_us.max(1)
            })
        }

        let n = spins_per_us();
        let mut i = 0u32;
        while i < n {
            std::hint::spin_loop();
            i = i.wrapping_add(1);
        }
    }

    /// Sleep until an absolute deadline by sleeping the remaining delta.
    fn sleep_until(deadline: std::time::Instant) {
        let now = std::time::Instant::now();
        if deadline > now {
            MonotonicClock::new().sleep(deadline - now);
        }
    }

    #[cfg(all(feature = "rt", target_os = "linux"))]
    fn setup_realtime() -> Result<(), String> {
        use libc::{
            MCL_CURRENT, MCL_FUTURE, SCHED_FIFO, mlockall, sched_param, sched_setscheduler,
        };

        // Try to set FIFO priority (requires CAP_SYS_NICE); ignore EPERM with warning upstream
        let mut param = sched_param { sched_priority: 10 };
        let rc = unsafe { sched_setscheduler(0, SCHED_FIFO, &mut param) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EPERM) {
                return Err(format!("sched_setscheduler failed: {err}"));
            }
        }

        let rc2 = unsafe { mlockall(MCL_CURRENT | MCL_FUTURE) };
        if rc2 != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EPERM) {
                return Err(format!("mlockall failed: {err}"));
            }
        }
        Ok(())
    }

    /// E-stop checker: on ARM, read from a GPIO and expose as closure.
    pub fn make_estop_checker(
        pin: u8,
        active_low: bool,
        poll_ms: u64,
    ) -> HwResult<Box<dyn Fn() -> bool + Send + Sync>> {
        use std::sync::atomic::AtomicBool;
        let gpio = Gpio::new().map_err(|e| HwError::Gpio(format!("open GPIO: {e}")))?;
        let pin = gpio
            .get(pin)
            .map_err(|e| HwError::Gpio(format!("get E-STOP pin: {e}")))?
            .into_input();
        let flag = Arc::new(AtomicBool::new(false));
        let flag_bg = flag.clone();
        thread::spawn(move || {
            let clock = MonotonicClock::new();
            loop {
                let level_low = pin.read() == rppal::gpio::Level::Low;
                let active = if active_low { level_low } else { !level_low };
                flag_bg.store(active, Ordering::Relaxed);
                clock.sleep(Duration::from_millis(poll_ms.max(1)));
            }
        });
        Ok(Box::new(move || flag.load(Ordering::Relaxed)))
    }
}

// Re-exports for callers (CLI/tests) to pick the right backend easily.
#[cfg(any(not(feature = "hardware"), not(target_os = "linux")))]
pub use sim::{SimulatedMotor, SimulatedScale};

#[cfg(all(feature = "hardware", target_os = "linux"))]
pub use hardware::{HardwareMotor, HardwareScale, make_estop_checker};

// Note: end-to-end pacing behavior is covered in the pacing::tests module using FakeSleeper.
