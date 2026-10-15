# 03: Tracker — thread loop, interruptible wait, latest, freeze, interval, stop

**What to build:** `tracker/mod.rs` + `tracker/worker.rs`. `Tracker::start(config) -> io::Result<Tracker>` normalizes the config, builds `Sampler::new()` on the caller's thread, spawns `thread::Builder::new().name(THREAD_NAME)` running `worker::run(sampler, shared, config)`; the `io::Error` of a refused thread is the only `Err`. `worker::Shared { control: Mutex<Control>, wake: Condvar, demand: AtomicUsize, latest: Mutex<Option<Arc<Reading>>>, subscribers: Mutex<Vec<SyncSender<Arc<Reading>>>> }` with `Control { interval, frozen, stopping, woken }` (seam 6); `Shared::wait(duration) -> Wait::{Elapsed, Woken, Stop}` loops on `wait_timeout` until the deadline, returning early when `stopping` or `woken` is set; `Shared::wake()` sets `woken` and notifies; `Shared::stop()` sets `stopping` and notifies. The loop is the plan's, verbatim; while frozen the sample is taken and discarded. `latest()`, `interval()`/`set_interval()` (clamped as in 01), `is_frozen()`/`set_frozen()`, `is_alive()` (`JoinHandle::is_finished`), `stop(self)` (signal + join), `Drop` (signal only) (seam 7). Locks are taken through one private `locked()` helper that recovers from poisoning. The demand and subscriber fields are declared here and used by 04/05.

**Blocked by:** 02.

**Status:** done

- [x] Tests first (live, `MIN_INTERVAL`): `latest()` is `None` right after `start`, then `Some` with `sequence() == 1` within 5 s; `chip().tiers` is non-empty on this machine.
- [x] Frozen for three intervals the sequence doesn't advance; thawed, the next reading's sequence is the old one plus one and its history is one longer.
- [x] `set_interval(Duration::ZERO)` reads back as `MIN_INTERVAL`; `set_interval(1 day)` as `MAX_INTERVAL`.
- [x] `is_alive()` is true while running; `stop()` returns within 2 s even when the interval is `MAX_INTERVAL`, and `is_alive()` is false afterwards.
- [x] Component + module + system suites green. Hand-off: `feat(telemetry): a tracker thread publishes readings at a steady interval`.
