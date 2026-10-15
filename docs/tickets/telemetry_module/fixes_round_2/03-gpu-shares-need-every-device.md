# 03: Per-process GPU shares are `Some` only when every GPU answered

**Finding:** `sample_shares` merged the devices that answered and returned `Some` when any did, so on a host with one GPU that answers the process query and one that refuses (MIG or vGPU beside a full device), a process on the refusing one was absent from the map and shown as `gpu: Some(0.0)`: the invented zero rule 1 forbids. Reproduced end to end with a fake two-device `libnvidia-ml.so.1`.

**What to build:** `nvml.rs`: `sample_shares` tracks whether every device answered and returns `None` otherwise (`merge_most` folds the answers); `GpuSample::process_shares` and the plan document "`Some` only when every GPU reported".

**Blocked by:** None.

**Status:** done

- [x] Code read; not reproducible here (no NVML). Verify on a mixed host when one is at hand.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): no GPU share is invented for a GPU that can't report`.
