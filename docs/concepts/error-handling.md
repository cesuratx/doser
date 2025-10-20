# Error Handling (this repo)

Where

- Libraries use `thiserror` (typed errors) and `eyre` in CLI.
- No `unwrap`/`expect` outside tests; errors bubble with context.

Patterns

- `Result<T, E>` with `?` for propagation.
- `AbortReason` is a domain enum (not just strings) → mapped to exit codes and JSONL.
- `color-eyre` in CLI for pretty human errors.

Java/.NET analogy

- `Result` ~ `Either<T, E>`; think checked exceptions but explicit in types.
- Use `?` like `throw;` but without stack-unwind cost unless error.

Snippet

```rust
fn load_cfg(path: &Path) -> eyre::Result<Config> {
    let s = std::fs::read_to_string(path)?;
    let cfg: Config = toml::from_str(&s)?;
    cfg.validate()?;
    Ok(cfg)
}
```

Notes

- CLI converts rich errors to one-line JSON when `--json` is set.
