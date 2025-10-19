# Configuration Schema

- `doser_config/src/lib.rs` defines `Config` and sub-structs: ControlCfg, FilterCfg, Safety, Timeouts, PredictorCfg, etc.
- `validate()` enforces bounds and cross-field invariants.
- Calibration CSV: strict header + robust refit to remove outliers before slope/intercept fit.
