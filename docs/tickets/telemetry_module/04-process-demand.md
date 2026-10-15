# 04: Process demand guards — wake, baseline, release

**What to build:** `ProcessDemand(Arc<Shared>)` in `types/structs.rs` with its `Drop` in `tracker/mod.rs`: `Tracker::demand_processes()` increments `Shared::demand` and, when it was 0, calls `Shared::wake()` so the loop runs now; `Drop` decrements. `Tracker::lists_processes()` reads the count. In the loop (`worker.rs`): `lists = demand > 0`; when it differs from `sampler.lists_processes()`, call `set_lists_processes(lists)` and, when turning on, `wait(PROCESS_BASELINE)` (ends early on stop only) before the sample, so process CPU rates have a span. The guard is `Send + Sync` (an `Arc` of atomics and mutexes) so a consumer thread can hold it.

**Blocked by:** 03.

**Status:** done

- [x] Tests first (live): with no guard the first reading has `processes: None`; after `demand_processes()` a reading with `processes: Some(non-empty)` arrives within 5 s and `busiest()` is non-empty; after dropping the guard a later reading has `processes: None` again.
- [x] Two guards: dropping one keeps `lists_processes()` true; dropping both makes it false.
- [x] The first guard shortens the wait: at a 1 h interval a reading still arrives within 5 s of the guard being taken.
- [x] Component + module + system suites green. Hand-off: `feat(telemetry): list processes only while someone asks`.
