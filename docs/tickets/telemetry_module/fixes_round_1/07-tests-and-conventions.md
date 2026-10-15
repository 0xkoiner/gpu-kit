# 07: Tests and conventions

**Findings:** `a_dropped_receiver_is_forgotten_at_the_next_publish` assumed no reading was queued before the drop (flaky under preemption); no tests for a changed interval's cadence, `Drop` letting the thread exit, or a NaN first value leaving `peak() == None`; rustdoc missing on `Tracker::is_frozen`, `Unit::Watts`, `Unit::BytesPerSecond`; bodies over 15 lines in `rank_busiest`, `Shared::wait`, `main.rs`'s `line` and `watch`; `THREAD_NAME` was 17 bytes and the kernel keeps 15, so `ps -L` showed `telemetry-sampl`; the vendoring recipe's `**` needs globstar; `--watch abc` printed a bare `ParseIntError`.

**What to build:** the flaky test waits for a sequence past `latest()` at the drop; the three tests join `test/tests.rs`; the rustdoc lands; `share()` leaves `rank_busiest`, `remaining()` leaves the waits, `emit`/`metric_entries`/`busiest_entries` leave `main.rs`; `THREAD_NAME = "metrics-sampler"` (15 bytes); the recipe uses `find … -exec sed`; `parse_millis` names the argument.

**Blocked by:** 01–06.

**Status:** done

- [x] Every `tracker/` body ≤ 15 lines; `cargo clippy -- -W missing_docs` reports nothing in `tracker/`.
- [x] Component + module + system suites green; clippy and rustdoc clean. Hand-off: `chore(telemetry): review round 1 tests and conventions`.
