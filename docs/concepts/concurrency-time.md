# Concurrency, Time, and Real-time hints

Where

- `doser_core/src/sampler.rs`: background sampling thread via channels.
- `crossbeam_channel` used for MPMC; non-blocking latest sample retrieval.
- `doser_traits/clock.rs`: `MonotonicClock` trait; test clock for determinism.
- RT helpers in CLI/hardware use `libc` (mlockall, sched\_\*), guarded with safety notes.

Patterns

- Spawn a thread that owns the scale; main loop reads from channel; optional paced loop with `Pacer`.
- Stall detection uses elapsed monotonic time vs configured thresholds.

Java/.NET analogy

- Thread + ConcurrentQueue + Stopwatch; pinning/affinity akin to setting thread priority and affinity.

Edge cases

- Channel disconnect (producer dropped): treat as stall/abort.
- Clock monotonicity: use provided Clock to avoid wall-clock jumps.
