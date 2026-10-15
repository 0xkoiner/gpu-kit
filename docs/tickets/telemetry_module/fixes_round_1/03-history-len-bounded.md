# 03: `history_len` is bounded by `MAX_HISTORY_LEN`

**Finding:** `TrackerConfig { history_len: usize::MAX, .. }` passed `start` with `Ok`, then `VecDeque::with_capacity` panicked with "capacity overflow" on the sampling thread: a silently dead tracker from a public config field, with no `unwrap` for the lint wall to catch.

**What to build:** `constants.rs`: `MAX_HISTORY_LEN = 86_400` (a day at 1 s; the history is cloned into every reading). `TrackerConfig::normalized` and `Series::new` clamp to `1..=MAX_HISTORY_LEN`; the field's rustdoc says so and why.

**Blocked by:** None.

**Status:** done

- [x] Test: `Series::new(usize::MAX).capacity() == MAX_HISTORY_LEN`.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): bound the history a reading carries`.
