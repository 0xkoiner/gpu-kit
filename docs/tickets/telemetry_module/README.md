# Tickets: `telemetry` module — the continuous metrics pipeline

**Sources (authority order):** **`docs/plan/modules/telemetry.md`** (the module contract these tickets implement) → the sampler layer as it stands in `src/modules/metrics/` (`Sampler`, `Snapshot`, the readers) → `bshk-app/top` `crates/app/src/monitor.rs` (the behaviour lifted: history, peak, busiest, freeze, demand-driven processes). In Photon, `CLAUDE.md`/`AGENTS.md` and `docs/plan/modules/tree.md` sit above all three on style and layout.
**Working agreements apply:** TDD (tests with the code, in `tracker/test/tests.rs`, none inline), lint wall active (no `unwrap`/`expect`/`panic`/indexing in source files), function bodies ≤ 15 lines, banners in every source file, rustdoc on every public item. In gpu-kit the operator is this session (commits land on `claude/linux-metrics`); in Photon the operator owns git and each ticket ends with a hand-off commit message.

## Seam decisions (recorded here once)

1. **The pipeline is a submodule of the metrics crate, not a crate:** `src/modules/metrics/tracker/`, re-exported from the crate root. The user's words: "keep it in the metric file". Vendoring into Photon copies the whole crate once.
2. **Data in `types/`, behaviour beside it:** `Series`, `History`, `Reading`, `TrackerConfig`, `ProcessDemand` are declared in `tracker/types/structs.rs`; their methods live in `history.rs`, `reading.rs` and `mod.rs`. Fields stay private except `TrackerConfig`'s, which a consumer fills with struct-update syntax.
3. **Gaps are `None`:** every series is `VecDeque<Option<f32>>`. No chart value is ever invented; `max`/`mean` skip gaps and are themselves `None` over an all-gap window.
4. **Peak lives in the series:** `Series::peak` is the largest value ever pushed, so the "Peak" a power table shows survives the value leaving the 48-sample window (top's `power_peak`). It is not reset by freeze.
5. **No indexing in the ranking:** `Reading::new` compares `processes.get(i).map_or(0.0, |p| p.cpu)` with `total_cmp` and `busiest()` yields `filter_map(get)`, so a malformed index can't panic (lint wall).
6. **One control block, one `Condvar`:** `worker::Shared` holds `Mutex<Control { interval, frozen, stopping, woken }>`, the demand `AtomicUsize`, `latest`, and the subscriber list. The thread's `wait` returns early only on `stop` and on the first `ProcessDemand`; an interval change is read at the next loop top.
7. **The host decides the life of the thread:** `stop(self)` joins; `Drop` signals without joining so a UI dropping its tracker never blocks. Poisoned locks yield their data (`unwrap_or_else(PoisonError::into_inner)`) rather than fail the host.
8. **Per-process GPU share is `Option`** (ticket 06): `Some` only when NVML reports process utilization; AMD and VMs give `None`, matching rule 1 of the plan. `GpuSample::process_shares` becomes `Option<HashMap<i32, f32>>` so the sampler can tell "no process used the GPU" from "nobody reports it".
9. **`--watch` is the system test, not a product:** `main.rs` keeps the one-shot dump as the default and adds streaming behind `--watch [ms]`; both compose their JSON with `serde_json::json!` from the public accessors, so `Reading` needs no `Serialize`.

## Order (dependency-first; work the frontier)

| # | Ticket | Blocked by |
|---|---|---|
| 01 | Series, History and Metric: the bounded window with gaps and peaks | — |
| 02 | Reading: the published unit, busiest ranking without indexing | 01 |
| 03 | Tracker: thread loop, interruptible wait, latest, freeze, interval, stop | 02 |
| 04 | Process demand guards: wake, baseline, release | 03 |
| 05 | Subscriptions: bounded per-subscriber channels, lossy publish | 03 |
| 06 | `Process.gpu` becomes `Option<f32>`; NVML threads `Option<HashMap>` | — |
| 07 | `--watch` streaming in `main.rs`, three suites green, plan bookkeeping | 01–06 |

01 and 06 start immediately. 02 after 01; 03 after 02; 04 and 05 after 03 and in parallel; 07 closes the set.

## Out of scope for this set

Reshaping the sampler files (`cpu.rs`, `process.rs`, …) to the module skeleton (banners, `types/`, `test/tests.rs`) · a Photon `telemetry` cargo feature and the `modules/mod.rs` gate · emitting readings as Photon metrics (the Product's `gauge!` calls read a `Reading`) · process control policy (SIGTERM→SIGKILL grace) · formatting (`"1.2 GB/s"`, `"≈2 %"`) · macOS.

## Vendoring recipe (Photon side, for when this set is done)

```
cp -r gpu-kit/src/modules/metrics photon/src/modules/telemetry
mv photon/src/modules/telemetry/lib.rs photon/src/modules/telemetry/mod.rs
# drop the Cargo.toml copied along; the crate attribute moves to the module decl
sed -i 's/crate::/crate::modules::telemetry::/g' photon/src/modules/telemetry/**/*.rs
sed -i 's/#\[cfg_attr(feature = "serde", derive(serde::Serialize))\]/#[derive(serde::Serialize)]/' photon/src/modules/telemetry/**/*.rs
# modules/mod.rs:  #[cfg(all(feature = "telemetry", target_os = "linux"))] pub mod telemetry;
```
