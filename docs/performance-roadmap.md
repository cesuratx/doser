# Performance Roadmap

This document explains how we will make dosing more "real-time" and why it matters. It is aimed at both developers and the product team.

## Goals

- Reduce end-to-end latency from sensor reading to motor command
- Reduce jitter (timing variability)
- Maintain or improve dosing accuracy (less overshoot)

## Phased Plan

### Phase 1 — Quick Wins (Low Risk)

- Release build tuning (Cargo profiles)
- Reduce/disable logging in the hot path
- Preallocate buffers and avoid heap allocations in the control loop
- Keep full speed until near target; apply fine_speed only in the final band

Expected benefit: 10–30% lower CPU time and more consistent timing.

### Phase 2 — Core Loop Efficiency

- Use static dispatch (generics) in hot paths to enable inlining
- Switch control math to fixed-point (centigrams) end-to-end

Expected benefit: Lower per-step latency and less rounding jitter → tighter stops.

### Phase 3 — Real-Time Sampling

- Dedicated sampler thread with bounded SPSC channel to the controller
- Event-driven updates on HX711 data-ready (DRDY) at 80 SPS; avoid sleeps

Expected benefit: Controller reacts immediately to new data → reduced overshoot.

### Phase 4 — OS and Hardware Scheduling

- Real-time scheduling (SCHED_FIFO) for sampler/control threads
- CPU affinity and performance governor on the Pi
- Hardware PWM for motor; O(1) stop/brake path

Expected benefit: Best determinism under load; protects against rare timing spikes.

## Acceptance Criteria per Phase

- Phase 1: Release build produces <1% heap allocs in control loop; no logs emitted at info in loop; dose time equal or faster than baseline with same accuracy.
- Phase 2: No f64 in hot path; microbench shows lower step latency; accuracy unchanged or improved.
- Phase 3: Sampler latency p95 < 5ms from DRDY to controller; overshoot distribution tightens.
- Phase 4: Under CPU stress, p99 step latency increases by <20% compared to idle.

## Risks & Mitigations

- Generics increase code size → limit type permutations.
- Fixed-point overflow → pick safe ranges (i32 centigrams covers ±21 kg).
- RT scheduling needs privileges → gate with feature flag and docs.

## Tracking Metrics

- Overshoot (grams), dose time (s), step latency (ms), CPU usage (%), p95/p99 jitter.

## Next Steps

- Implement Phase 1 (profiles + logging gates) and benchmark.
