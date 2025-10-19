# Data Flow (ASCII)

```
Config (TOML) ──> doser_config::Config.validate()
                      │
                      ▼
Hardware (Scale/Motor via doser_hardware) ── implements ──> doser_traits
                      │
                      ▼
Runner (doser_core::runner::run)
  ├── Direct: read scale synchronously
  ├── Event: spawn Sampler thread → crossbeam_channel
  └── Paced: Pacer tick loop
                      │
                      ▼
Control loop (DoserCore)
  ├── Filters: EMA/MA/Median
  ├── Predictor: early stop using slope_ema + inflight
  └── Safety: timeouts, bounds, estop
                      │
                      ▼
Outcome
  ├── Success: final_g, duration_ms, telemetry
  └── Abort: AbortReason → exit code
                      │
                      ▼
CLI Output
  ├── Human text (default)
  └── JSONL (--json) stable schema
```
