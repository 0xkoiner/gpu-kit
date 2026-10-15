# 05: Subscriptions — bounded per-subscriber channels, lossy publish

**What to build:** `Tracker::subscribe() -> mpsc::Receiver<Arc<Reading>>`: `sync_channel(UPDATE_QUEUE)`, the sender pushed onto `Shared::subscribers`. `Shared::publish(reading)` sets `latest` and then, for each sender, `try_send(reading.clone())`: `Ok` and `Err(Full)` keep the sender (the reading is simply missed by that subscriber), `Err(Disconnected)` drops it (`retain`). Publish never blocks the sampling thread (seam 6, plan rule 3).

**Blocked by:** 03.

**Status:** done

- [x] Tests first (live): a subscriber taken before the first tick receives sequences 1, 2, 3 in order; `latest()` after the third has `sequence() >= 3`.
- [x] A subscriber that never reads doesn't stall a second subscriber: after `UPDATE_QUEUE + 4` publishes the second has received everything and the tracker is alive.
- [x] Dropping a receiver drops its sender at the next publish (observed through `tracker.subscriber_count()`, a `#[cfg(test)]` observation seam in the ADR 0004 sense).
- [x] Component + module + system suites green. Hand-off: `feat(telemetry): subscribers receive each reading on a bounded channel`.
