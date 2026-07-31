# Configuration Schema

- `doser_config/src/lib.rs` defines `Config` and sub-structs: `Pins`, `FilterCfg`,
  `ControlCfg`, `Timeouts`, `Safety`, `Logging`, `Hardware`, `PredictorCfg`, `EstopCfg`,
  `RunnerCfg`, `PersistedCalibration`.
- `[pins]`, `[filter]` and `[timeouts]` are **required**; every other table is `#[serde(default)]`.
- `validate()` enforces bounds and rejects non-finite floats; window sizes are capped.
- Calibration CSV: strict header + robust refit to remove outliers before slope/intercept fit.
  A persisted `[calibration]` table in the TOML takes precedence over the CSV at runtime.
- Full key-by-key reference (required vs defaulted, real defaults, deprecated keys):
  [reference/CONFIG_SCHEMA.md](../reference/CONFIG_SCHEMA.md).
