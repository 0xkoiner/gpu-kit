# 04: NVML: a GPU that refuses the process query gives `None`, not an empty map

**Finding:** "the driver reports shares" was decided by whether the `nvmlDeviceGetProcessUtilization` symbol resolved, not by whether the call succeeded. A GPU answering `NOT_SUPPORTED` (MIG, vGPU, some consumer cards) yielded `Some(empty)`, so every process got `gpu: Some(0.0)`: the invented zero rule 1 of the plan forbids, and a promise `gpu.rs`'s doc made without the code behind it.

**What to build:** `nvml.rs`: `process_shares` returns `Option<HashMap>`: `Some(empty)` on `NOT_FOUND` or a zero count (nobody used the GPU), `None` on any other failing status or a failed read; `sample_shares` merges the devices that answered and is `None` when none did; `mean_shares` is split out so each body stays short.

**Blocked by:** None.

**Status:** done

- [x] Code read: `Some(0.0)` is now produced only when some GPU answered the query. Not reproducible on this machine (no NVML); verify on an NVIDIA host with `cargo run` and a MIG or vGPU device if one is at hand.
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): a GPU that can't report process shares reports none`.
