# Logging & JSONL

- Tracing: `tracing` initialized in CLI; logs to stderr.
- JSONL: `--json` makes stdout emit one JSON object per line with stable keys:
  - timestamp, target_g, final_g, duration_ms, profile, slope_ema, stop_at_g, coast_comp_g, abort_reason
- Integration tests assert schema, ensuring logs on stderr won’t corrupt JSONL.
