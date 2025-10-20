# Traits & Generics (repo-specific)

Where it appears

- `doser_traits`: `Scale`, `Motor`, `MonotonicClock` define behavior.
- `doser_core/src/lib.rs`: `DoserCore<S: Scale, M: Motor>` is generic; `Doser` is the trait-object wrapper (`Box<dyn Scale>`).
- `doser_hardware`: two impl families: simulated and hardware, both implement the same traits.

Why

- Let core be testable and platform-agnostic: pass in any `Scale`/`Motor`/`Clock`.

Java/.NET analogy

- Trait ~ interface.
- Generic `DoserCore<S,M>` ~ `class DoserCore<TScale, TMotor> where TScale : IScale`.
- `Box<dyn Scale>` ~ `IScale` reference with dynamic dispatch.

Key call sites

- `build_doser` accepts either concrete generics (unit tests) or trait objects (CLI path).
- `runner::run` assembles hardware/sim objects and passes them as trait objects.

Snippet

```rust
pub struct DoserCore<S: Scale, M: Motor> {
    scale: S,
    motor: M,
    // ...
}

pub struct Doser {
    inner: DoserCore<Box<dyn Scale>, Box<dyn Motor>>,
}
```
