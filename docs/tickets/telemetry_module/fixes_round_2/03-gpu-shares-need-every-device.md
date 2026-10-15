# 03: Per-process GPU shares are `Some` only when every GPU answered

**Finding:** `sample_shares` merged the devices that answered and returned `Some` when any did, so on a host with one GPU that answers the process query and one that refuses (MIG or vGPU beside a full device), a process on the refusing one was absent from the map and shown as `gpu: Some(0.0)`: the invented zero rule 1 forbids. Reproduced end to end with a fake two-device `libnvidia-ml.so.1`.

**What to build:** `nvml.rs`: `sample_shares` collects every device's answer and `merge_all` folds them, `None` when any GPU didn't answer or there is none; `GpuSample::process_shares` and the plan document "`Some` only when every GPU reported".

**Blocked by:** None.

**Status:** done (host check open)

- [x] Unit tests on `merge_all` (round 3): a process keeps the most it took of any GPU; one `None` answer makes the whole `None`; two idle GPUs are `Some(empty)`; no GPU is `None`.
- [ ] Verified on a mixed NVIDIA host (a MIG or vGPU device beside a full one) with `cargo run`: processes on the refusing device show `gpu: null`. No NVML here; the round-2 finder's fake two-device library reproduced the defect, not the fix.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): no GPU share is invented for a GPU that can't report`.
