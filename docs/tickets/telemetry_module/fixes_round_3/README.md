# Fixes round 3: `telemetry` module (convergence check)

**Source:** an adversarial review of HEAD `1844c06` (fixes round 2) limited to what round 2 changed: two finders (lifecycle and semantics; tests and lint wall), two skeptics per finding, probes under the scratchpad. One finding was refuted, three survived; **none is a code defect**, so the review loop is dry after this round.

## The three, all closed in one commit

| # | Item | Severity | What changed |
|---|---|---|---|
| 01 | `a_guard_released_before_it_is_seen_ends_listing_at_the_scheduled_tick` could pass with its fix reverted in 1–3 % of runs: taking and dropping guard B wakes the worker before B drops, so the worker sometimes saw B alive | low (test strength) | the test produces the state directly with `shared.wake()` after dropping A, as the round-2 stale-wake test does; a reverted fix now fails every run |
| 02 | The plan's loop pseudo-code (`if ended == Woken and not lists: continue`) disagreed with the code, which also continues when a new guard replaces an old one while listing is on; following the pseudo-code would reintroduce the sub-span sample rounds 1 and 2 removed | low (contract) | pseudo-code now reads `if ended == Woken and not started: continue`; the `demand_processes` row and rustdoc say a guard taken while the sampler still lists waits for the scheduled tick (`latest` already holds a list); a pinning test `a_guard_taken_while_still_listing_waits_for_the_scheduled_tick` |
| 03 | Round-2 ticket 03 ticked "verify on a mixed host" without the check, and the every-GPU fold had no executable coverage | low (record) | `merge_all` is a pure fold with four unit tests in `nvml.rs`; the ticket's host check is an open box |

## Refuted

"A re-demand while the sampler still lists takes no early sample, against the plan": the mechanism is real and deliberate since round 1 (`latest` already carries a list at most one interval old; sampling at once would be the confirmed sub-span defect); the plan was the side out of step, fixed by item 02.

## Found sound

Every demand interleaving (0→1→0, 1→0→1) against `wait_tick`/`pause`/sample/publish resolves to continue-the-tick or a full-span `Started` sample (min gap between readings under 4 s of random toggling at `MIN_INTERVAL`: 98.8 ms); `Follow::Ended` on `Elapsed` and `Woken` both leave the scheduled sample with `processes: None`; 200 `stop`/`subscribe` races: no hang, late subscriber born disconnected; `--watch` first reading carries processes then a steady cadence; NVML statuses map as documented; the lint wall (unwrap, expect, panic, unreachable, todo, indexing, arithmetic side effects, string slicing, missing docs) reports nothing in `tracker/`; bodies at most 15 lines; 44 banner frame lines byte-identical; the suite passed five times including serial and under eight CPU burners.
