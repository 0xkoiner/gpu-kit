# Photon module plan: `telemetry` — continuous system metrics

**Status:** delivered on branch `claude/linux-metrics` (tickets 01–07 done, see `docs/tickets/telemetry_module/`); written in gpu-kit for vendoring into Photon as `src/modules/telemetry/` behind a `telemetry` feature.
**Authority:** this plan → `docs/tickets/telemetry_module/README.md` (seam decisions) → the tickets. In Photon the chain above it is `CLAUDE.md`/`AGENTS.md` → `docs/plan/modules/tree.md`; where they disagree with this file on layout or style, they win.
**Reference:** `bshk-app/top`'s `crates/app/src/monitor.rs` is the behaviour being lifted out of a GPUI entity into a library pipeline; its views (tiles, vitals, process panel) are not.

## Purpose

One background thread samples the machine (CPU, GPU, power, temperatures, memory, network, disk, volumes, battery, processes) at a steady interval and publishes, after every sample, an immutable `Reading`: the `Snapshot` plus what any dashboard derives from it — a bounded history per metric, peaks since start, and the busiest processes. Consumers pull the latest reading or receive each one on a channel. Nothing here renders, formats, or emits Photon signals; the host (a UI, or Photon's own emit paths) does that from a `Reading`.

Rules carried over from the samplers and kept by the pipeline:

1. **A metric the machine doesn't provide is `None`, never 0.** The history keeps `Option<f32>` so a gap stays a gap; means and maxima skip gaps.
2. **Rates span the time between two samples.** The first sample after `Tracker::start` already has rates because `Sampler::new` takes the baseline.
3. **Derived values are computed once, on the sampling thread.** Readers never rank, average or copy on their own.
4. **No panics, no `unwrap`/`expect`/indexing** in source files (Photon's lint wall): a poisoned lock yields its data, an index that can't exist is skipped.

## Placement in Photon

- Vendored whole: today's `src/modules/metrics/` becomes `src/modules/telemetry/`; `lib.rs` becomes `mod.rs` (its `#![cfg(target_os = "linux")]` moves onto the `pub mod telemetry;` line in `modules/mod.rs`, next to `#[cfg(feature = "telemetry")]`); every `crate::` path in the module becomes `crate::modules::telemetry::` (one `sed`); the `serde` cargo feature becomes unconditional (`cfg_attr(feature = "serde", …)` → plain derives, since Photon's core always has serde).
- No new dependencies: the pipeline uses `std` threads, `Mutex`/`Condvar`, atomics and `mpsc`. Photon may swap `Mutex<Option<Arc<Reading>>>` for `ArcSwapOption<Reading>` and `std::sync::Mutex` for `parking_lot::Mutex` when vendoring; the surface doesn't change.
- Photon's `signals/metric` (counters and gauges a Product registers) is unrelated: `telemetry` measures the host, not the Product.

## Public surface

All items are re-exported from the module root (`lib.rs` here, `mod.rs` in Photon). Types from the existing snapshot layer (`Snapshot`, `Process`, `ChipInfo`, `Sampler`, …) are unchanged except where ticket 06 says.

| Item | Contract |
|---|---|
| `TrackerConfig { interval: Duration, history_len: usize, busiest_len: usize }` + `Default` | Defaults are the constants below. `interval` is clamped to `[MIN_INTERVAL, MAX_INTERVAL]`, `history_len` to at least 1; `busiest_len` may be 0. |
| `Tracker::start(config) -> io::Result<Tracker>` | Builds the `Sampler` on the caller's thread (loads the GPU library, opens RAPL, takes the rate baselines — a moment, so call it off a UI thread), then spawns the `telemetry-sampler` thread. `Err` only when the OS refuses a thread. |
| `Tracker::chip(&self) -> &ChipInfo` | Static facts of the machine, known from `start`. |
| `Tracker::latest(&self) -> Option<Arc<Reading>>` | The newest reading; `None` until the first interval has elapsed. A clone of an `Arc`, never of the data. |
| `Tracker::subscribe(&self) -> mpsc::Receiver<Arc<Reading>>` | Every reading published from now on, in order, on a queue of `UPDATE_QUEUE`. A subscriber that falls that far behind misses readings rather than stalling the sampler; a dropped receiver is forgotten at the next publish. |
| `Tracker::interval(&self) -> Duration` / `set_interval(&self, Duration)` | The wait between samples. A change applies from the next tick. |
| `Tracker::is_frozen(&self) -> bool` / `set_frozen(&self, bool)` | Frozen, the thread keeps sampling so rates stay fresh, but publishes nothing: `latest`, history, peaks and `sequence` stand still until thawed. |
| `Tracker::demand_processes(&self) -> ProcessDemand` | Processes are listed (the most expensive part of a sample) only while at least one guard lives. The first guard wakes the thread at once; it records the process baseline, waits `PROCESS_BASELINE`, then samples. When the last guard drops, the next reading has `processes: None`. |
| `Tracker::lists_processes(&self) -> bool` | Whether a guard is alive. |
| `Tracker::is_alive(&self) -> bool` | Whether the sampling thread is still running. |
| `Tracker::stop(&self)` | Signals the thread and joins it; `is_alive()` is false afterwards. Callable through an `Arc`. `Drop` only signals; the thread then exits within one wait. |
| `Reading` | `sequence() -> u64` (1 for the first published reading, +1 per publish; unchanged while frozen) · `taken_at() -> SystemTime` · `snapshot() -> &Snapshot` · `history() -> &History` · `busiest() -> impl Iterator<Item = &Process>` (by CPU, most first, at most `busiest_len`, empty when processes aren't listed). |
| `History` | `new(capacity)` · `push(&Snapshot)` (one value per `Metric`) · `series(Metric) -> &Series` · `len()` · `capacity()` · `is_empty()`. |
| `Series` | `new(capacity)` · `push(Option<f32>)` (drops the oldest at capacity) · `values() -> impl Iterator<Item = Option<f32>>` oldest first · `latest()` · `max()` and `mean()` over the window's present values · `peak()` the largest value ever pushed, which outlives the window · `len()` · `capacity()` · `is_empty()`. |
| `Metric` | `Cpu`, `Gpu`, `Memory`, `Power`, `NetworkReceive`, `NetworkSend`, `DiskRead`, `DiskWrite` · `ALL` · `read(self, &Snapshot) -> Option<f32>` · `unit(self) -> Unit` · `label(self) -> &'static str`. |
| `Unit` | `Percent` (0–100), `Watts`, `BytesPerSecond`. |

`Metric::read` is the one place a snapshot turns into a chart value: CPU is `Cpu::usage()`, GPU is `gpu.usage`, memory is `used / total × 100` (`None` when `total` is 0 or memory unread), power is `Power::total()`, the four rates are `Throughput`'s fields as `f32`.

## Constants (`tracker/types/constants.rs`)

| Name | Value | Why |
|---|---|---|
| `DEFAULT_INTERVAL` | 1 s | top's default refresh. |
| `MIN_INTERVAL` | 100 ms | Below this /proc reads dominate the machine they measure. |
| `MAX_INTERVAL` | 1 h | Keeps `Condvar::wait_timeout` arithmetic in range. |
| `HISTORY_LEN` | 48 | top's sparkline width. |
| `BUSIEST_LEN` | 5 | top's popover list. |
| `PROCESS_BASELINE` | 250 ms | Process CPU rates need a span after listing resumes. |
| `UPDATE_QUEUE` | 8 | Readings a subscriber may lag before it misses some. |
| `THREAD_NAME` | `"telemetry-sampler"` | What `ps -L` and panics show. |

## The loop (`tracker/worker.rs`)

```
loop:
    wait(interval)                      — ends early on stop, or when the first ProcessDemand opens
    if stopping: break
    lists = demand_count > 0
    if lists != sampler.lists_processes():
        sampler.set_lists_processes(lists)
        if lists: wait(PROCESS_BASELINE)   — also ends early on stop
    snapshot = sampler.sample()
    if frozen: continue                  — the sample kept the rate baselines fresh; nothing else
    history.push(&snapshot); sequence += 1
    reading = Arc::new(Reading::new(sequence, snapshot, history.clone(), busiest_len))
    publish(reading)                     — latest = reading; each subscriber: try_send, drop on Full, forget on Disconnected
```

The wait is one `Condvar` on the control block; wakes are explicit (`stop`, first demand). An interval change is read at the top of the next loop, so it takes effect within one old interval, as in top.

## Layout (`src/modules/metrics/tracker/`, Photon skeleton)

```
tracker/
├── mod.rs              banners · submodule decls · `#[cfg(test)] #[path = "test/tests.rs"] mod tests;` ·
│                       Tracker: start/stop/latest/subscribe/interval/freeze/demand/is_alive · Drop
├── worker.rs           the loop above; the Shared control block (Mutex<Control> + Condvar, demand count,
│                       latest, subscribers) and its wait/wake/publish
├── history.rs          Series::push/latest/max/mean/peak · History::push/series (behaviour; data in types/)
├── reading.rs          Reading::new (busiest ranking by select_nth_unstable_by, no indexing) · accessors
├── types/
│   ├── mod.rs
│   ├── structs.rs      TrackerConfig · Reading · History · Series · ProcessDemand (data + Default)
│   ├── enums.rs        Metric (+ ALL, read, unit, label) · Unit
│   └── constants.rs    the table above
└── test/
    └── tests.rs        tickets 01–05's tests; nothing inline in source files
```

Existing files (`cpu.rs`, `process.rs`, `snapshot.rs`, …) keep their shape; reshaping them to the skeleton is a separate ticket set, not this one.

## Tests

- **Component** (`tracker/test/tests.rs`, pure): series window and peak, mean/max over gaps, `Metric::read` for each metric and each `None` path, busiest ranking (20 processes → the five highest by CPU, descending; `processes: None` → empty; `busiest_len` 0 → empty; NaN doesn't panic), config clamping.
- **Module** (same file, live, Linux): `Tracker::start` at `MIN_INTERVAL`: a subscriber receives sequence 1 within 5 s and `latest()` agrees; frozen for three intervals, `sequence` doesn't move, thaw and it does; `demand_processes` turns `processes` from `None` to `Some` within the next readings and back to `None` after the guard drops; `stop()` returns and `is_alive()` is false; a dropped subscriber doesn't stop the others.
- **System** (`cargo run -- --watch 500`): one JSON line per reading with sequence, timestamp, every metric's value, peak and mean, and the busiest processes; `cargo run` alone still writes the one-shot `metrics.json`.

## Out of scope

Rendering and formatting (tiles, sparklines, `format::rate` strings) · Photon signal emission (the Product's `gauge!` wiring reads a `Reading`; Photonville's) · process control (`send_signal` stays a free function; top's SIGTERM→SIGKILL grace is UI policy) · persistence of history across runs · a `telemetry` feature flag and `Cargo.toml` edits in Photon · reshaping the sampler files to the module skeleton · macOS.
