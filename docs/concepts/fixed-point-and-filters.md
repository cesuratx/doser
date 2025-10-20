# Fixed-point & Filters (overview)

- Internals operate in centigrams (cg) as integers for thresholds and comparisons.
- Filters: moving average, median window, EMA; configured via `FilterCfg`.
- Predictor uses EMA slope + inflight compensation.

Deep dive: docs/concepts/fixed-point-filters.md
