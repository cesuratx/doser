# Enums, Matching, and State

Where it appears

- `doser_core/src/error.rs`: `AbortReason` guides exit codes and JSONL output.
- `doser_core/src/runner.rs`: `SamplingMode` selects run strategy.
- `doser_core/src/lib.rs`: control state transitions (e.g., dosing vs. early stop) use enums and `match`.

Why it matters

- Pattern matching is the control-center for decisions: exhaustive checks avoid forgotten cases.

Java/.NET analogy

- Rust `enum` ~ C# discriminated unions (OneOf, DU in F#) or Java sealed interfaces with variants.
- `match` ~ switch expression but exhaustive and value-destructuring.

Snippet

```rust
match sampling_mode {
    SamplingMode::Direct => run_direct(...),
    SamplingMode::Event => run_with_sampler(...),
    SamplingMode::Paced(period) => run_paced(period, ...),
}
```
