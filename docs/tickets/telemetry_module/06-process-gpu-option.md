# 06: `Process.gpu` becomes `Option<f32>`; NVML threads `Option<HashMap>`

**What to build:** `snapshot.rs`: `Process.gpu: Option<f32>`, documented as "`Some` where a driver reports per-process GPU time (NVML); `None` elsewhere". `nvml.rs`: `Nvml::sample` returns `(Vec<DeviceSample>, Option<HashMap<i32, f32>>)`, `None` when `process_utilization` is unsupported, otherwise `Some` (an empty map means no process used the GPU). `gpu.rs`: `GpuSample.process_shares: Option<HashMap<i32, f32>>`, `None` for AMD; `aggregate` passes it through. `process.rs`: `ProcessTable::sample(memory_total, gpu_shares: Option<&HashMap<i32, f32>>)` sets `gpu: gpu_shares.map(|shares| shares.get(&pid).copied().unwrap_or(0.0))`. `lib.rs`: `Sampler::sample` and `set_lists_processes` pass the option. `main.rs` keeps sorting by `cpu`; JSON gains `null` for the unknown share (seam 8).

**Blocked by:** None.

**Status:** done

- [x] Tests first: `aggregate` keeps `None` shares for AMD-shaped input and `Some(empty)` for an NVML sample with no busy process.
- [x] A live `Sampler::sample` on this machine (no NVML) lists processes whose `gpu` is `None`.
- [x] `cargo run` reports `snapshot.processes[*].gpu` as `null` here; the "not read" summary doesn't list it (lists aren't walked).
- [x] Component + module + system suites green. Hand-off: `fix(telemetry): an unreported process GPU share is None, not 0`.
