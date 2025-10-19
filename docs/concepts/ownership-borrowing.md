# Ownership & Borrowing (in this repo)

Where it appears

- `doser_traits/src/lib.rs`: trait object indirection (Box<dyn Scale/Motor>) avoids borrowing across threads.
- `doser_core/src/lib.rs`: `DoserCore<S,M>` owns `scale` and `motor` by value; `Doser` owns a `DoserCore<Box<dyn Scale>, Box<dyn Motor>>`.
- `doser_core/src/sampler.rs`: thread moves `scale` into the sampler thread, so it must be owned (Send + 'static), not borrowed.

What it is (Rust)

- Ownership ensures a single owner of data; borrowing uses `&`/`&mut` references with lifetimes.
- We prefer ownership or boxed trait objects to keep lifetimes simple across threads and crates.

Java/.NET analogy

- Ownership ~ a single strong reference with deterministic drop; borrowing ~ passing references that must not outlive the owner.
- Box<dyn Trait> ~ interface reference (dynamic dispatch) with heap allocation.

Why we use it here

- The sampler and hardware threads need `'static` ownership; borrowing `&mut Scale` across threads would complicate lifetimes. Owning the device (by value or Box) keeps APIs simple and safe.

Reading checklist

- In `DoserCore<S,M>`, confirm `scale: S, motor: M` are owned fields.
- In builder, note `with_scale(self, scale: impl Scale + 'static)` converts to `Box<dyn Scale>` for the dynamic `Doser` API.
- In `Sampler::spawn`, see `move` closure taking ownership of `scale`.

Snippet

```rust
// doser_core/src/sampler.rs
std::thread::spawn(move || {
    loop {
        match scale.read(timeout) { /* ... */ }
    }
});
```
