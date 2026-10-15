# 02: Subscribers disconnect when the thread ends

**Finding:** the `SyncSender`s lived in `Shared.subscribers`, kept alive by the `Tracker` itself, so after `stop()` or a worker panic a `recv()` blocked forever; `for reading in tracker.subscribe()` in `--watch` would hang silently on a dead thread.

**What to build:** `worker.rs`: `struct Exit<'a>(&'a Shared)` whose `Drop` clears the subscriber list, held for the whole of `run`, so a return and a panic both disconnect every receiver. `Tracker::subscribe` documents it. Dead senders still accumulate only while frozen (no publish prunes them); recorded, not fixed: std has no disconnection probe short of a send.

**Blocked by:** None.

**Status:** done

- [x] Test: `subscribe(); stop();` then `recv_timeout` is `Disconnected`, not `Timeout`.
- [x] Test: `Drop` of a tracker at `MAX_INTERVAL` releases the shared block within 5 s (the thread exited).
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): receivers disconnect when the sampler thread ends`.
