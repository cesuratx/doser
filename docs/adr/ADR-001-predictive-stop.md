# ADR-001: Predictive Stop in Control Loop

Context

- Mechanical latency and inflight mass cause overshoot. We need predictable target achievement within tolerance without excessive slow finish.

Decision

- Add Predictor using EMA slope of mass and an inflight compensation to trigger early stop when forecasted final mass reaches target within tolerance.
- Expose `PredictorCfg` in `doser_config` and plumb into `doser_core` builder and `runner`.

Consequences

- More parameters in config; tests must validate ranges and defaults.
- Control loop must export telemetry for slope, inflight, and stop_at for observability.
- Failure modes include mis-tuned predictor → abort rules still protect via timeouts/safety bounds.

Status

- Accepted.
