# 01: The process baseline pause heeds `stop` alone; a stale wake takes no sample

**Finding:** `follow_demand` reused `Shared::wait` for the 250 ms baseline, so a `wake()` (guard A dropped and guard B taken within the span, or a wake racing a naturally elapsed tick) ended it at once and the next sample divided process CPU ticks by a span of microseconds: a probe saw a process at 525 % CPU. A wake whose guard was already gone also made the next wait return immediately and take a second sample milliseconds after the first, pushing quantized values and spiky rates into the history and the never-decaying peaks.

**What to build:** `worker.rs`: `Shared::pause(duration) -> bool` that loops on `wait_timeout` checking `stopping` only; `follow_demand` uses it and reports `Follow::{Unchanged, Changed, Stopped}`; `next_sample` treats `Woken` + `Unchanged` as "the tick goes on where it was" and waits again toward the same deadline instead of sampling. `Tracker::start` turns listing off before spawning, so the first guard's wake is the first state change.

**Blocked by:** None.

**Status:** done

- [x] Test: at `MAX_INTERVAL`, guard A, 60 ms, drop A, guard B: the reading with processes arrives no earlier than `PROCESS_BASELINE` after A.
- [x] Test: a wake with no guard behind it (`shared.wake()`, what a guard taken and dropped before the thread looks leaves) at `MAX_INTERVAL` produces no reading within 1 s and the thread stays alive (round 2 made it deterministic).
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): a second guard can't cut the process baseline short`.
