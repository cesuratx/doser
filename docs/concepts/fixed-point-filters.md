# Fixed-point math & Filters

Where

- `doser_core/src/lib.rs`: grams ↔ centigrams conversion; EMA/MA/median filters.
- Predictor uses EMA slope and inflight mass to decide early stop.

Why

- Deterministic math and avoiding float drift in control loop.

Details

- Store mass in centigrams (i32/i64) for stable comparisons.
- Filters configurable via `FilterCfg`.

Snippet

```rust
fn to_cg(g: f32) -> i32 { (g * 100.0).round() as i32 }
```

Notes

- Keep conversions at boundaries (I/O and JSONL), not inside inner loops.
