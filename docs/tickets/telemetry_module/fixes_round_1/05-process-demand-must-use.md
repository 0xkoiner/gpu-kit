# 05: `ProcessDemand` is `#[must_use]`

**Finding:** `tracker.demand_processes();` as a statement compiled silently; the guard dropped at the semicolon, readings kept `processes: None`, and the stale wake of ticket 01 fired.

**What to build:** `types/structs.rs`: `#[must_use = "processes are listed only while the guard lives"]` on `ProcessDemand`.

**Blocked by:** None.

**Status:** done

- [x] `cargo build` warns on the bare statement.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): a dropped-at-once demand guard warns`.
