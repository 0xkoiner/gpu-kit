# 07: `--watch` streaming in `main.rs`, three suites green, plan bookkeeping

**What to build:** `src/main.rs`: `cargo run -- --watch [interval_ms]` starts a `Tracker` with that interval (default `DEFAULT_INTERVAL`), holds one `ProcessDemand`, and for each reading from `subscribe()` prints one JSON line: `sequence`, `taken_at` (seconds since the epoch), `interval_secs`, for every `Metric` an object `{ value, mean, peak, unit }` keyed by `label()`, `busiest: [{ pid, command, cpu, memory }]`, and the thermal pressure label when present; stops on EOF of stdout (`BrokenPipe`) or Ctrl-C. The default (no flag) one-shot dump is unchanged (seam 9). Then: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo doc --no-deps`, a 3-second `--watch 500` run showing sequences 1–5, and the plan's status line updated.

**Blocked by:** 01–06.

**Status:** done

- [x] `cargo run -- --watch 500 | head -5` prints five lines, sequences 1–5, each parseable JSON with `cpu.value` a number and `busiest` non-empty.
- [x] `cargo run` still writes `metrics.json` and prints the "not read" summary.
- [x] Component (`cargo test -p top-metrics tracker`), module (`cargo test -p top-metrics`), system (`cargo test --workspace` + the run above) green; clippy and rustdoc clean.
- [x] Hand-off: `feat(telemetry): stream readings as JSON lines with --watch`.
