# Fixes round 2: `telemetry` module

**Source:** an adversarial review of HEAD `f144d66` (fixes round 1): three independent finders (concurrency and lifecycle; data semantics; lint wall, conventions and tests) each hunted the fixed code, and every finding went to two skeptics asked to refute it with a probe. Six findings were raised; four survived both skeptics (one raised by two finders), one was refuted (test bodies over 15 lines: the rule covers source files), and the finders' "found sound" notes cover lost wakeups, nested locks, the demand counter, panic paths, NaN handling, ranking preconditions, banners and the lint wall.
**Working agreements apply:** as for the set (`../README.md`).

## Order

| # | Ticket | Severity | Blocked by |
|---|---|---|---|
| 01 | A receiver taken after the thread ended is born disconnected | medium | — |
| 02 | A stale wake takes no sample even when listing turns off (round 1 fix 01 completed) | medium | — |
| 03 | Per-process GPU shares are `Some` only when every GPU answered | low | — |
| 04 | The stale-wake test wakes without a guard, so it can't race the worker | low | 02 |

All four landed in one commit.

## Found sound (recorded so the next round needn't re-prove it)

No lost wakeups (`stopping`/`woken`/`interval` are read under the control lock before every `wait_timeout`); no nested locks, so `stop()`'s join can't deadlock; `publish` never blocks; the demand counter is balanced (`ProcessDemand` isn't `Clone`); the baseline `pause` is a full span; `Drop` never blocks; no panic path on the thread (clamped capacities, saturating and checked arithmetic, guarded `select_nth_unstable_by`, total comparator, poisoned locks recovered). `Series`/`History`/`Metric::read`/`Reading::new` match the plan including NaN and gaps; no sampler produces NaN or infinity. The lint wall (unwrap, expect, panic, unreachable, indexing, arithmetic side effects, string slicing, missing docs) reports nothing in `tracker/` source files; all 44 banner frame lines are byte-identical; ten full test runs, serial and 16-way parallel, passed.
