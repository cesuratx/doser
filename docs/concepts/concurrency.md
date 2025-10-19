# Concurrency (channels & threads)

- Sampler thread owns `Scale` and pushes readings to a channel.
- Main loop receives latest sample without blocking; handles disconnect as abort.
- Use `crossbeam_channel` for MPMC + select timeouts.

See also: docs/concepts/concurrency-time.md for RT hints.
