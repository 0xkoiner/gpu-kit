# 02: A stale wake takes no sample even when listing turns off

**Finding:** round 1 skipped a stale wake only when `follow_demand` reported `Unchanged`. A guard taken and dropped while the thread was in the baseline pause, the sample or the publish left `woken` set; at the next `wait_tick` the demand was 0 but the sampler still listed, so `follow_demand` switched listing off, reported `Changed`, and the thread sampled at once: a second reading 0.5–0.8 ms after the first, with tick-quantized CPU (0 % or 50 % on this machine) and spiky rates pushed into every series and the never-decaying peaks. Reproduced 4/4 and 6/6 by two probes; any panel flicker within ~300 ms of a first demand hits it.

**What to build:** `worker.rs`: `Follow::{Unchanged, Started, Ended, Stopped}`; `next_sample` continues the tick on `Woken` for `Unchanged` and `Ended` alike (listing is already off; the scheduled sample carries `processes: None` over a full span); only `Started`, whose baseline pause guarantees a span, samples early.

**Blocked by:** None.

**Status:** done

- [x] Test: at `MAX_INTERVAL`, listed reading, drop A, take and drop B: no reading within 1 s; after `set_interval(MIN_INTERVAL)` the next reading has `processes: None`.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): a stale wake never samples, whichever way listing moved`.
