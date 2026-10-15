# 04: The stale-wake test wakes without a guard, so it can't race the worker

**Finding:** `let _ = tracker.demand_processes();` increments the demand and notifies before the guard drops, so about 1 run in 1000 (measured over 5 000 wakes) the worker saw the demand, listed, and published inside the test's 1 s window: a flake on a CI that runs the suite often.

**What to build:** `test/tests.rs`: the test calls `tracker.shared.wake()` directly (the state a dropped-at-once guard leaves behind) and is renamed `a_wake_with_no_guard_behind_it_takes_no_sample`; fixes round 1 ticket 01's checkbox says so.

**Blocked by:** 02.

**Status:** done

- [x] Ten runs, serial and parallel, pass.
- [x] Hand-off: `test(telemetry): a stale wake is produced without a guard`.
