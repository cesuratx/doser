# Time & Determinism

Where

- `doser_traits::clock::MonotonicClock` abstracts time; `TestClock` used in tests.
- `doser_core::runner` computes timeouts, stall thresholds, and durations using monotonic time.

Rules

- Never use wall-clock for control; prefer monotonic.
- Convert all timeouts to Duration once; avoid repeated conversions in hot loops.

In tests

- Use `TestClock` to simulate passage of time deterministically.
