# 01: Series, History and Metric — the bounded window with gaps and peaks

**What to build:** `tracker/types/{mod,structs,enums,constants}.rs` + `tracker/history.rs`. `Series::new(capacity)` over `VecDeque<Option<f32>>` with `push` dropping the oldest at capacity and raising `peak` for a `Some`; `values()` oldest first; `latest()` (the last pushed value, `None` for a gap or an empty series); `max()`/`mean()` over the window's present values, `None` when there are none; `peak()` since the first push, outliving the window (seam 4); `len`/`capacity`/`is_empty`. `History::new(capacity)` holds one `Series` per `Metric` and `push(&Snapshot)` feeds each from `Metric::read` (seam 3); `series(Metric)` via `match`, no array indexing. `Metric` (`Cpu`, `Gpu`, `Memory`, `Power`, `NetworkReceive`, `NetworkSend`, `DiskRead`, `DiskWrite`) with `ALL`, `read(&Snapshot) -> Option<f32>` exactly as the plan defines each, `unit() -> Unit`, `label()`. `TrackerConfig` with `Default` from the constants and a `pub(crate) fn normalized(self) -> Self` that clamps `interval` to `[MIN_INTERVAL, MAX_INTERVAL]` and `history_len` to at least 1. The constants table of the plan, verbatim.

**Blocked by:** None.

**Status:** done

- [x] Tests first: pushing 49 values into a 48-series keeps the newest 48 and `peak()` still returns the dropped first value when it was the largest.
- [x] `mean()`/`max()` skip `None`; an all-gap window gives `None`; `latest()` is `None` right after a gap is pushed.
- [x] `Metric::read`: memory `used/total×100`, `None` when `total == 0` or memory is unread; power is `Power::total()`; the four rates map `read`/`write`; CPU `None` without cores; GPU `None` without a driver.
- [x] `History::push` grows every series by one; `len()` is the shortest series' length, `capacity()` the configured one.
- [x] `TrackerConfig::normalized` clamps a 1 ms interval up and a 2 h interval down; `history_len` 0 becomes 1.
- [x] Component + module + system suites green. Hand-off: `feat(telemetry): bounded metric history with gaps and peaks`.
