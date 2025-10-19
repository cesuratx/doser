# Unsafe & OS Integration

- RT helpers in CLI use libc: mlockall, sched_setscheduler, sched_setaffinity; macOS/Linux supported paths.
- Each unsafe call has documented invariants and returns Result.
- Prefer feature-gated RT elevation in hardware step thread.
