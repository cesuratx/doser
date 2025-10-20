# Glossary (Rust ↔ Java/.NET, plus domain/OS terms)

Core Rust mappings

- Trait → Interface
- Generic → Generic (where constraints)
- Enum (tagged union) → Sealed hierarchy / DU (F#)
- Result<T,E> → Either / OneOf / Try
- Box<dyn Trait> → Interface reference on heap (dynamic dispatch)
- Send/Sync → Thread-safety markers for threads
- MonotonicClock → Stopwatch-like, monotonic time source (no wall-clock jumps)
- OnceLock → Thread-safe, write-once holder (Lazy-like)

IO/Formats/Config

- Serde/TOML → Serde is Rust’s (de)serialization framework; TOML is the config format parsed by `toml`
- JSONL → One JSON object per line; stable keys, easy to stream/grep

Filtering & math

- EMA (Exponential Moving Average) → Smoothing filter: s*t = α·x_t + (1−α)·s*{t−1}; fast, emphasizes recent samples (see docs/concepts/fixed-point-filters.md)
  - Why here: stabilizes noisy scale readings and provides a slope for predictive stop
- MA (Moving Average) → Arithmetic mean of last N samples; reduces noise but lags
- Median Filter → Middle value of a sliding window; removes spikes/outliers
- Slope (EMA slope) → Estimated grams-per-second trend, often from smoothed series; used by predictor (see docs/adr/ADR-001-predictive-stop.md)
- Fixed-point (centigrams) → Store mass as integer centigrams (cg = 0.01 g) for deterministic comparisons
- Saturating arithmetic → Adds/subtracts but clamps at min/max to avoid overflow
- Round-to-nearest, ties-away-from-zero → Rounding policy used in integer helpers

Calibration & statistics

- OLS (Ordinary Least Squares) → Linear fit grams = a·raw + b minimizing squared residuals (see doser_config/src/lib.rs)
- Residual / RMS → Residual: observed − predicted; RMS: root-mean-square magnitude of residuals
- Robust refit → One-pass refit after rejecting points with |residual| > k·RMS; reduces outlier influence (see docs/CONFIG_SCHEMA.md and doser_config/src/lib.rs)
- Welford/Chan updates → Numerically stable online mean/covariance updates used in refit

Control & safety

- Predictor → Early-stop forecaster: uses slope (trend) and inflight mass to stop motor before target to reduce overshoot (see docs/adr/ADR-001-predictive-stop.md and docs/concepts/control-loop.md)
  - Why here: reduces overshoot without crawling too slowly near the target
- Inflight mass → Material expected to arrive after motor stop due to latency/inertia
- Hysteresis → A small band around a threshold to avoid rapid toggling
- Epsilon (ε) → Tiny tolerance (e.g., epsilon_g) used for approach/finish decisions
- E‑stop (Emergency stop) → Hard stop input; debounced and latched by controller
- Debounce (N) → Require N consecutive pressed samples before latching E‑stop
- Stall watchdog → Timer that aborts if samples stop arriving for longer than a threshold

Timing & concurrency

- Monotonic duration/Instant → Time measured by a monotonic clock; immune to wall-clock changes
- Pacer → Absolute-deadline ticker that regulates loop timing and measures jitter (see doser_hardware/src/lib.rs::pacing)
  - Why here: keeps paced sampling/stepping consistent when no DRDY is available
- crossbeam_channel → MPMC channel used for sampler thread to send latest readings (see doser_core/src/sampler.rs)
  - Why here: decouples sensor thread from controller loop with a small, non-blocking buffer

Hardware & sensors

- HX711 → 24‑bit ADC for load cells (scale). Has a DRDY (data-ready) pin (see doser_hardware/src/lib.rs)
  - Why here: primary scale interface; event-driven sampling when on hardware
- DRDY (Data Ready) → Sensor line asserts when a new sample is ready (event-driven sampling)

OS/real‑time primitives (unsafe/privileged on Linux/macOS)

- mlockall → Lock process memory into RAM (no paging). Flags (see docs/concepts/unsafe-os.md and doser_cli/src/main.rs):
  - MCL_CURRENT: lock all current pages; MCL_FUTURE: also lock future allocations
  - Needs CAP_IPC_LOCK or sufficient ulimit (RLIMIT_MEMLOCK). Reduces page-fault jitter
  - Why here: avoids page faults during runs that could cause latency spikes and overshoot
- sched_setscheduler (SCHED_FIFO) → Put current thread/process into real-time FIFO policy at a priority (needs CAP_SYS_NICE/root) (see docs/concepts/unsafe-os.md)
  - Why here: reduces preemption by best-effort real-time scheduling on supported systems
- sched_setaffinity → Bind to a specific CPU to reduce migration jitter; CPU_ZERO/CPU_SET/CPU_ISSET manage masks (see docs/concepts/unsafe-os.md)
  - Why here: CPU pinning can reduce jitter in tight control loops
- RLIMIT_MEMLOCK/getrlimit → OS limit that caps how much memory can be locked with mlock/mlockall (see docs/concepts/unsafe-os.md)

Errors & testing libraries

- thiserror → Derive macro for typed error enums in libraries
- eyre / color‑eyre → Ergonomic error reports in binaries (pretty reports, backtraces)
- proptest → Property‑based testing
- cargo‑fuzz (libFuzzer) → Coverage‑guided fuzzing for parsers/validators
- criterion → Micro‑benchmarking framework
- tarpaulin → Code coverage for Rust (Linux)
