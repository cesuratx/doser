# Rust Primer for This Project

This primer explains the Rust language features and idioms used in this repository, with small, practical examples from the codebase. It aims to help you read and extend the code confidently.

If you’re new to Rust, keep The Rust Book handy: https://doc.rust-lang.org/book/

---

## 1) Crates, Modules, Workspaces, and Features

- Crate: A package (library or binary). This repo is a workspace of multiple crates: `doser_core`, `doser_cli`, `doser_hardware`, `doser_traits`, `doser_config`, `doser_ui`.
- Module: Organizes code inside a crate via `mod` and files.
- Workspace: A set of crates built together for faster, consistent builds. See top-level `Cargo.toml`.
- Feature flags: Compile-time toggles. We use `hardware` to include real GPIO implementations. Conditional compilation is done with `#[cfg(feature = "hardware")]` and platform gating with `#[cfg(any(target_arch = "arm", target_arch = "aarch64"))]`.

Example (from `doser_hardware`):

- On ARM: include rppal and real GPIO code.
- On non-ARM: still compile the crate, but skip rppal code; provide stubs so CI passes.

Why: Keep tests/CI green without hardware while allowing real builds on devices.

---

## 2) Ownership, Borrowing, and Lifetimes (in practice)

- Ownership: Each value has one owner. When you `move` a value, the old binding becomes invalid.
- Borrowing: Use references `&T` (shared) or `&mut T` (exclusive) without taking ownership.
- Lifetimes: Tell the compiler how long references are valid. Most are inferred automatically.

In this project, we move hardware implementations into the core `Doser` so it has exclusive control. That avoids data races and ensures clean motor shutdown in `Drop`.

---

## 3) Traits, Generics, and Trait Objects

- Trait: A set of required methods (an interface). Example in `doser_traits`:
  - `Scale` with `read(&mut self, timeout: Duration) -> Result<i32, Box<dyn Error + Send + Sync>>`
  - `Motor` with `start`, `set_speed`, `stop`
- Generics: Write code that works with any type implementing a trait.
- Trait objects: `Box<dyn Trait>` for runtime polymorphism when the concrete type isn’t known at compile time.

Where we use them:

- The core `Doser` is generic over `Scale` and `Motor` but often boxes them to store in a single struct and erase the concrete type.
- For E‑stop, we use `Box<dyn Fn() -> bool + Send + Sync>` — a boxed closure (callable) that can be swapped at runtime.

Why `Box`? Trait objects have unknown size at compile time; `Box` stores them on the heap with a stable pointer.

Object safety: Only object-safe traits can be used behind `dyn Trait`. Fn traits are object-safe; many normal traits are too if they follow object-safety rules.

---

## 4) `Send` and `Sync` marker traits

- `Send`: Safe to move between threads.
- `Sync`: Safe to share references between threads.

We bound the E‑stop closure with `+ Send + Sync` because it may be created by a background thread (GPIO poller) and read from the control loop. Adding these bounds prevents accidental use of non-thread-safe types.

---

## 5) Common Smart Pointers: `Box`, `Arc`, `Rc`, `Mutex`

- `Box<T>`: Heap allocation, used for trait objects or large values. We use it for `Box<dyn Fn() -> bool + Send + Sync>`.
- `Arc<T>`: Atomically Reference Counted pointer for thread-safe sharing. In the hardware E‑stop checker, we use `Arc<AtomicBool>` to share a flag between a background thread and the main thread.
- `Rc<T>`: Non-atomic reference counting, single-threaded only. Not used here.
- `Mutex<T>`: Mutual exclusion for interior mutability across threads. Not needed yet; we prefer message passing or atomics where possible.

---

## 6) Closures and `move`

Closures capture variables from the environment. The `move` keyword forces captures by value. Example:

- Hardware E‑stop factory spawns a thread that updates an `Arc<AtomicBool>` and returns a closure that reads it. We `move` the `Arc` into the closure so it can outlive the function that created it.

Why: Ownership must be clear. `move` ensures the closure owns its captures, avoiding dangling references.

---

## 7) Enums, Pattern Matching, `Option` and `Result`

- `Enum`: A type with multiple variants. We use `DosingStatus::{Running, Complete, Aborted(DoserError)}`.
- `Option<T>`: `Some(T)` or `None`. Used for optional config values like E‑stop pin.
- `Result<T, E>`: `Ok(T)` or `Err(E)`. Used pervasively for fallible operations.
- Pattern matching with `match` drives state machines elegantly.

In the CLI dose loop, we `match` on `DosingStatus` to continue, report completion, or handle aborts.

---

## 8) Error Handling: `anyhow` vs `thiserror` and Domain Errors

- Libraries (`doser_core`): define a domain error enum (e.g., `DoserError`) using `thiserror` for readable Display/From impls. This gives typed, testable errors.
- Applications (`doser_cli`): use `anyhow` for ergonomic propagation and context strings.
- Mapping: Hardware-specific errors (e.g., timeouts) are mapped centrally to domain variants (e.g., `DoserError::Timeout`). Tests assert correct mapping without string matching.

Tip: Use `.with_context(|| ...)` (anyhow) to enrich errors at the edges (file I/O, parsing).

---

## 9) RAII and `Drop`

Resource Acquisition Is Initialization: resources clean themselves up when dropped.

We implement a `Drop` behavior (inside `Doser`) to attempt to stop the motor when the controller goes out of scope, providing a safety net in case of early returns or errors.

---

## 10) Builder Pattern and Method Chaining

The `Doser::builder()` returns a builder that consumes and returns `self` on each method:

- Encourages immutable configuration objects and a single final `build()`.
- Makes invalid states unrepresentable by validating at build time (e.g., window sizes > 0).

Why methods take ownership: moving values into the builder prevents later accidental reuse.

---

## 11) Conditional Compilation: `cfg` and `target_arch`

- `#[cfg(feature = "hardware")]`: include code only when the `hardware` feature is enabled.
- `#[cfg(any(target_arch = "arm", target_arch = "aarch64"))]`: include code only on ARM systems.
- Combining them lets us compile the “hardware” feature on x86 CI without pulling in rppal, while enabling real GPIO on ARM.

Also see `Cargo.toml` target-specific dependencies to include `rppal` only on ARM.

---

## 12) Concurrency Building Blocks Used

- `std::thread::spawn`: start a background thread for polling (E‑stop checker).
- `std::sync::Arc`: share state between threads.
- `std::sync::atomic::AtomicBool`: lock-free boolean for a simple flag.
- Sleep/poll loop with `std::time::Duration` for simple periodic tasks.

This is intentionally simple and robust; no async runtime is required for the current scope.

---

## 13) Logging with `tracing`

- Spans/Events: We primarily use events (`tracing::info!`, `warn!`, `error!`).
- Layers: Console (pretty or JSON) and optional file layer with rotation.
- Non-blocking writer: `tracing_appender::non_blocking` returns a writer and a guard; we keep the `WorkerGuard` in a `OnceLock` so it lives for the process duration.

Why: Structured logs aid debugging and operations; non-blocking avoids stalling the control loop under heavy logging.

---

## 14) Testing and Determinism

- Unit tests assert filtering, calibration, safety guards, and E‑stop latch.
- A `Clock` trait lets tests control time without sleeping, making tests fast and deterministic.
- Integration tests exercise the CLI and error mapping paths.

Tip: Prefer dependency injection (traits, clocks) to isolate time and I/O in tests.

---

## 15) Useful Syntax and Patterns You’ll See

- `?` operator: Propagate errors concisely: `fn foo() -> Result<T, E> { bar()?; Ok(...) }`.
- Method chaining: `.with_filter(filter).with_control(control)`.
- `unwrap_or`, `unwrap_or_else`: Provide defaults for `Option` or `Result` when safe.
- `to_ascii_lowercase()` and matching strings for config options.
- `match` with guards and multiple arms for clear control flow.

---

## 16) Performance Notes

- Trait objects (`Box<dyn Trait>`) use dynamic dispatch; the overhead is negligible here versus I/O times. For tight loops, generics/monomorphization can be faster, but at the cost of more complex types.
- Filtering windows are small; allocations are minimized. Consider preallocations if windows grow.

---

## 17) Extending the Project Safely

- Add new hardware by implementing `doser_traits::{Scale, Motor}` and wiring it behind a feature.
- Keep error mapping centralized to preserve consistent behavior.
- Validate new config fields in the core builder.
- Prefer pure functions and small structs for testability.

---

## 18) Quick Reference: When to Use What

- Use a trait when multiple types share behavior (e.g., real vs simulated scale).
- Use a trait object (`Box<dyn Trait>`) when you need to store heterogeneous implementors in a single field.
- Use generics when the type is known at compile-time and you want zero-cost abstraction.
- Use `Arc<AtomicBool>` to share a simple flag across threads; use `Mutex<T>` for shared mutable data that’s not atomic.
- Use `#[cfg(...)]` to isolate platform-specific or optional code.

---

## 19) Further Reading

- The Rust Book: https://doc.rust-lang.org/book/
- Error handling: anyhow + thiserror patterns — https://nick.groenen.me/posts/rust-error-handling/
- Conditional compilation: https://doc.rust-lang.org/reference/conditional-compilation.html
- Concurrency: https://doc.rust-lang.org/book/ch16-00-concurrency.html
- Tracing: https://docs.rs/tracing and https://docs.rs/tracing-subscriber

---

With these building blocks, you should be able to navigate the code, understand the tradeoffs, and add features with confidence.
