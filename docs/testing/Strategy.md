# Testing Strategy

Layers

- Unit tests in each crate for pure logic (filters, predictor, conversions).
- Integration tests for runner/control paths and CLI JSONL schema.
- Property tests (proptest) for config validation and numeric invariants.
- Fuzzing (cargo-fuzz) for TOML parser + validator.
- Benches (Criterion) for predictor and hot paths.

Coverage

- Use tarpaulin for line/branch coverage; exclude hardware feature-gated code.

Guidelines

- No `unwrap`/`expect` outside tests.
- Prefer deterministic `TestClock`.
- Assert JSONL keys and types, not formatting.
