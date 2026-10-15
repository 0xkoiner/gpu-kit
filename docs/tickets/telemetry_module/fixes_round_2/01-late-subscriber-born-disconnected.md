# 01: A receiver taken after the thread ended is born disconnected

**Finding:** the `Exit` guard cleared the sender list once, but `subscribe` kept pushing new senders afterwards and nothing ever dropped them, so `stop(); subscribe(); recv()` blocked forever, and a `subscribe` racing `stop` on another thread hung in 8–18 of 40 probe runs. A UI that re-subscribes on a view rebuild during shutdown, or `for reading in tracker.subscribe()` after an unobserved sampler panic, hangs silently.

**What to build:** `worker.rs`: `Subscribers { senders, closed }` under the one lock; `Exit::drop` sets `closed` and clears in the same critical section; `subscribe` pushes only while open, otherwise lets the sender drop so the receiver is born disconnected. Plan and rustdoc say so.

**Blocked by:** None.

**Status:** done

- [x] Test: `stop(); subscribe(); recv_timeout` is `Disconnected` and `subscriber_count()` is 0.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): a subscriber after shutdown is born disconnected`.
