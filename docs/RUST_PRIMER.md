# Rust Primer for This Project

This primer explains the Rust language features and idioms used in this repository, with small, practical examples from the codebase. It aims to help you read and extend the code confidently.

If you’re new to Rust, keep The Rust Book handy: https://doc.rust-lang.org/book/

Note: For a system-level overview and diagrams, see [ARCHITECTURE](./ARCHITECTURE.md).

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

---

## 20) Design Patterns Used in This Codebase

This section maps familiar software design patterns to concrete spots in the repo so you can recognize and extend them confidently.

- Adapter

  - What: Wrap one interface and present another.
  - Where: `doser_hardware::hardware::HardwareScale` wraps an `Hx711` driver and implements `doser_traits::Scale`. `HardwareMotor` wraps GPIO (rppal) and implements `doser_traits::Motor`.
  - Why: Keeps `doser_core` hardware-agnostic; lets us swap real vs simulated hardware without touching the control loop.

- Builder

  - What: Stepwise construction with validation at `build()`.
  - Where: `Doser::builder()` with `.with_scale()`, `.with_motor()`, `.with_filter()`, `.with_control()`, `.with_safety()`, `.with_timeouts()`, `.with_target_grams()`, optional `.with_estop_check()`, `.with_clock()`.
  - Why: Encourages immutable config and makes invalid states unrepresentable via early validation.

- Dependency Injection (DI)

  - What: Supply dependencies from the outside, not hardcoded.
  - Where: `Scale`, `Motor`, `Clock`, and E‑stop checker closure are injected into `Doser` via the builder.
  - Why: Improves testability (simulation + test clock) and portability (hardware feature gating).

- Factory

  - What: A function constructs and returns a ready-to-use object/closure.
  - Where: `doser_hardware::hardware::make_estop_checker(pin, active_low, poll_ms)` builds a polled GPIO checker and returns `Box<dyn Fn() -> bool + Send + Sync>`.
  - Why: Encapsulates platform details (GPIO) behind a simple call.

- Facade

  - What: A thin orchestrator that hides subsystem complexity.
  - Where: `doser_cli` parses config/args, initializes logging, selects hardware vs simulation, wires E‑stop, and runs the dose.
  - Why: Gives end-users a simple entry point without exposing internal crates.

- State Machine (lightweight)

  - What: Logic progresses through states with explicit transitions.
  - Where: `Doser::step()` returns `DosingStatus::{Running, Complete, Aborted(..)}` and manages settle windows, latching E‑stop, and watchdogs.
  - Why: Clear control flow that is easy to test and reason about.

- Strategy (extensibility point)

  - What: Pluggable algorithms under a common interface.
  - Where: The current controller uses parameterized coarse/fine speeds and hysteresis. The examples show how to plug in alternative strategies by driving `Doser::step()` yourself. A dedicated `DosingStrategy` trait can be added later if multiple algorithms must coexist.
  - Why: Allows experimenting with different dosing approaches without invasive changes.

- RAII / Drop Safety Net

  - What: Resources clean up themselves on scope exit.
  - Where: `Doser` and `HardwareMotor` attempt to stop the motor in `Drop`. `HardwareMotor` also joins its worker thread and disables EN (active‑low) on drop.
  - Why: Provides a safety backstop during errors or early returns.

- Feature/Plugin Toggle (compile-time)

  - What: Compile-time switches to include/exclude implementations.
  - Where: `#[cfg(feature = "hardware")]` gates hardware code; simulation is the default. `rppal` is an optional dependency pulled only with the `hardware` feature.
  - Why: Keeps CI/platforms without GPIO building cleanly while enabling real hardware on Raspberry Pi.

- Layered Logging

  - What: Compose multiple sinks/encoders.
  - Where: `doser_cli::init_tracing` builds console (pretty/JSON) and optional file layers with rotation, using a non-blocking writer and a global guard.
  - Why: Production-friendly logging without blocking the control loop.

- Error Typing and Mapping
  - What: Single domain error surface plus mapping.
  - Where: `doser_core::DoserError` is the domain error; hardware errors are mapped into it so callers don’t depend on platform details.
  - Why: Predictable behavior in both code and tests.

## 21) Concurrency Patterns in HardwareMotor

- Worker Thread + Atomics

  - `HardwareMotor` spawns a background thread that owns the STEP pin and generates pulses. Control signals (`running`, `sps`) are shared via `Arc<AtomicBool>` / `Arc<AtomicU32>`.
  - The main thread sets direction and optional enable (active‑low), and adjusts `sps` via atomics. A channel provides a shutdown signal on drop.

- Practical Notes
  - Move non-clonable GPIO pins (like STEP) into the worker thread; keep DIR/EN on the main thread for control.
  - Clamp step rate to a safe upper bound (~5 kHz here) and use coarse sleeps (`std::thread::sleep(Duration::from_micros(..))`) rather than busy loops.
  - Treat EN as active‑low (low = enabled) for common drivers (A4988/DRV8825). Default to disabled on startup and disable on drop.

## 22) Testing Patterns Used

- Deterministic Time

  - Inject a test clock implementing `doser_core::Clock` to advance time deterministically in unit tests.

- Simulation-First

  - Most tests use `SimulatedScale`/`SimulatedMotor`, keeping CI fast and platform-independent.

- Health Check CLI
  - `SelfCheck` subcommand probes scale read and motor start/stop without moving the mechanism, suitable for bring-up and automation.

---

## 23) Visual Overview (Primer)

A compact view of how crates and components interact during a dose.

```mermaid
flowchart LR
  subgraph Inputs
    A[CLI args]\nTOML config\nCalibration CSV
  end
  A --> B[doser_cli]
  B --> C[Doser::builder]
  C -->|injects| D[Filter/Control/Safety/Timeouts/Clock]
  C -->|with_scale/with_motor| E{Backend}
  E -->|hardware| H[HardwareScale & HardwareMotor]
  E -->|simulation| S[SimulatedScale & SimulatedMotor]
  C --> G[Doser]
  G --> L[step(): read -> filter -> safety -> motor]
  G --> R[DosingStatus: Running/Complete/Aborted]
  B -->|logs| X[tracing: console + optional file]
```

See also: [ARCHITECTURE](./ARCHITECTURE.md) for a deeper discussion.

---

## 24) Handy Cross‑links

- Traits: `../doser_traits/src/lib.rs`
- Core controller: `../doser_core/src/lib.rs`
- Hardware backends: `../doser_hardware/src/lib.rs`
- CLI entrypoint: `../doser_cli/src/main.rs`
- Examples: `../examples/`
- Architecture doc: `./ARCHITECTURE.md`

---

## 25) Type‑State (Type‑Checked) Builder

We use a type‑state builder so the compiler guarantees that required fields are provided before `build()` is available.

- Idea: Encode progress in type parameters (`Missing` vs `Set`). Each setter flips a marker.
- Result: `build()` only exists for `Builder<Set, Set, Set>` (scale, motor, target present).

Mini example adapted to our Doser builder:

```rust
struct Missing; struct Set;
struct Builder<S, M, T> { /* fields... */ _s: PhantomData<S>, _m: PhantomData<M>, _t: PhantomData<T> }

impl<S, M, T> Builder<S, M, T> {
    fn with_scale(self, s: ScaleImpl) -> Builder<Set, M, T> { /* ... */ }
    fn with_motor(self, m: MotorImpl) -> Builder<S, Set, T> { /* ... */ }
    fn with_target_grams(self, g: f32) -> Builder<S, M, Set> { /* ... */ }
    // optional setters keep the markers unchanged
}

impl Builder<Set, Set, Set> {
    fn build(self) -> Doser { /* validated construction */ }
}
```

Why this helps here

- Prevents constructing a `Doser` without hardware or target grams at compile time.
- Keeps optional fields (filter, control, safety, timeouts, clock, E‑stop) flexible and order‑agnostic.

Tradeoffs

- Slightly more generic boilerplate and type signatures.
- IDE type hints can be noisier, but method chaining remains ergonomic.
