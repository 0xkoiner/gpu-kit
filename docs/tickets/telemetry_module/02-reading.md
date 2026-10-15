# 02: Reading — the published unit, busiest ranking without indexing

**What to build:** `tracker/reading.rs` on the `Reading` struct of `types/structs.rs` (`sequence: u64`, `taken_at: SystemTime`, `snapshot: Snapshot`, `busiest: Vec<usize>`, `history: History`; all private). `pub(crate) fn new(sequence, snapshot, history, busiest_len) -> Reading` ranks `snapshot.processes` by `cpu` descending: indices `0..len`, `select_nth_unstable_by` when more than `busiest_len`, then `truncate` and `sort_by`, the comparator reading `processes.get(i).map_or(0.0, |p| p.cpu)` with `f32::total_cmp` (seam 5). Accessors `sequence()`, `taken_at()`, `snapshot()`, `history()`, and `busiest() -> impl Iterator<Item = &Process>` as `filter_map(|&i| processes.get(i))`. `taken_at` is `SystemTime::now()` at construction.

**Blocked by:** 01.

**Status:** done

- [x] Tests first: 20 processes with `cpu = ix * 7 % 20` rank to `[19, 18, 17, 16, 15]` at `busiest_len` 5 (top's test, kept).
- [x] `processes: None` and `busiest_len == 0` both give an empty `busiest()`; fewer processes than `busiest_len` gives all of them, sorted.
- [x] A `NaN` CPU share neither panics nor lands above a real number.
- [x] `sequence()` and `history()` return what was passed; `taken_at()` is not before the test started.
- [x] Component + module + system suites green. Hand-off: `feat(telemetry): readings rank the busiest processes once`.
