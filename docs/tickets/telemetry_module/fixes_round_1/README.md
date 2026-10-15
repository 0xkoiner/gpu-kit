# Fixes round 1: `telemetry` module

**Source:** an independent review of HEAD `70987e6` (tickets 01–07 delivered), run with the code and the plan only. Findings were verified by a throwaway probe crate before they became tickets; each ticket names the plan rule it restores.
**Working agreements apply:** as for the set (`../README.md`).

## Order (dependency-first; work the frontier)

| # | Ticket | Severity | Blocked by |
|---|---|---|---|
| 01 | The process baseline pause heeds `stop` alone; a stale wake takes no sample | high | — |
| 02 | Subscribers disconnect when the thread ends | medium-high | — |
| 03 | `history_len` is bounded by `MAX_HISTORY_LEN` | medium | — |
| 04 | NVML: a GPU that refuses the process query gives `None`, not an empty map | medium | — |
| 05 | `ProcessDemand` is `#[must_use]` | medium | — |
| 06 | `set_interval` applies to the wait in progress | medium-low | 01 (same `wait`) |
| 07 | Tests and conventions: flaky subscriber test, missing tests, rustdoc, 15-line bodies, thread name, recipe | low | 01–06 |

All seven landed in one commit; the tests of 01, 02, 03 and 06 are the regression tests the review asked for.
