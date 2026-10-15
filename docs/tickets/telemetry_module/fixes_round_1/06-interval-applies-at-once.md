# 06: `set_interval` applies to the wait in progress

**Finding:** the interval was read once per wait, so shortening from 1 h to 1 s took up to an hour to deliver a reading: a trap for the UI host the plan names.

**What to build:** `worker.rs`: `wait_tick(tick)` recomputes the deadline from `tick` and the current interval on every pass of its loop; `set_interval` notifies the condvar. The deadline moves without an extra sample (unlike reusing `wake`). Plan and rustdoc say "applies at once".

**Blocked by:** 01 (same `wait`).

**Status:** done

- [x] Test: at `MAX_INTERVAL`, `set_interval(MIN_INTERVAL)` delivers sequence 1 within 5 s.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): an interval change moves the wait in progress`.
