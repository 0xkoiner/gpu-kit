//! Linux system metrics for the UI, read from /proc and /sys, with NVML (the
//! NVIDIA driver's library) or the amdgpu driver's sysfs files for the GPU.
//! No crate dependencies (the `serde` feature adds `Serialize` to the snapshot
//! types) and no root, apart from RAPL's energy counters, which recent kernels
//! open to root alone. A metric the machine doesn't provide is `None`.
#![cfg(target_os = "linux")]

mod amdgpu;
mod cpu;
mod disk;
mod files;
mod gpu;
mod memory;
mod network;
mod nvml;
mod power;
mod process;
mod rate;
mod snapshot;
mod sys;
mod system;
mod thermal;
mod volumes;

use std::collections::HashMap;

pub use process::{Signal, SignalError, is_running, send_signal};
pub use snapshot::*;

/// Owns every counter whose rate needs a previous reading. Rates cover the
/// time between two `sample` calls, so the first call has none (`None`);
/// [`Sampler::new`] makes that call.
pub struct Sampler {
    chip: ChipInfo,
    cpu: cpu::CpuTicks,
    gpus: Option<gpu::Gpus>,
    rapl: Option<power::Rapl>,
    processes: process::ProcessTable,
    lists_processes: bool,
    network: network::NetworkCounters,
    disk: disk::DiskCounters,
}

impl Sampler {
    /// Loads the GPU driver's library, if there is one, and opens RAPL. Both
    /// take a moment, so construct this off the UI thread.
    pub fn new() -> Self {
        let gpus = gpu::Gpus::open();
        let gpu_cores = gpus.as_ref().and_then(gpu::Gpus::cores).unwrap_or(0);
        let mut sampler = Self {
            chip: system::chip_info(gpu_cores),
            cpu: cpu::CpuTicks::default(),
            gpus,
            rapl: power::Rapl::open(),
            processes: process::ProcessTable::new(),
            lists_processes: true,
            network: network::NetworkCounters::default(),
            disk: disk::DiskCounters::default(),
        };
        // Record baselines so the first visible sample already has rates.
        sampler.sample();
        sampler
    }

    pub fn chip(&self) -> &ChipInfo {
        &self.chip
    }

    /// Whether `sample` lists processes, its most expensive part.
    pub fn lists_processes(&self) -> bool {
        self.lists_processes
    }

    /// Starts or stops listing processes. Starting records a baseline for
    /// their CPU rates, so wait a moment before the next `sample`.
    pub fn set_lists_processes(&mut self, lists: bool) {
        if lists && !self.lists_processes {
            self.processes
                .sample(self.chip.memory_bytes, &HashMap::new());
        }
        self.lists_processes = lists;
    }

    pub fn sample(&mut self) -> Snapshot {
        let core_usage = self.cpu.sample();
        let cores = cpu::label_cores(&core_usage, &self.chip.tiers);
        let gpu = self.gpus.as_mut().and_then(gpu::Gpus::sample);
        let energy = self
            .rapl
            .as_mut()
            .and_then(power::Rapl::sample)
            .unwrap_or_default();
        let power = Power {
            cpu: energy.cpu,
            gpu: gpu.as_ref().and_then(|gpu| gpu.watts),
            dram: energy.dram,
            system: energy.system.or_else(system::battery_power),
        };
        let no_shares = HashMap::new();
        let gpu_shares = gpu.as_ref().map_or(&no_shares, |gpu| &gpu.process_shares);

        Snapshot {
            cpu: Cpu {
                cores,
                clocks: cpu::tier_clocks(&self.chip.tiers),
            },
            gpu: gpu.as_ref().map(|gpu| Gpu {
                usage: gpu.usage,
                mhz: gpu.mhz,
            }),
            power,
            temperatures: Temperatures {
                cpu: thermal::cpu_temperature(),
                gpu: gpu.as_ref().and_then(|gpu| gpu.celsius),
            },
            thermal: thermal::pressure(),
            memory: memory::read(),
            network: self.network.sample(),
            disk: self.disk.sample(),
            volumes: volumes::read(),
            battery: system::battery(),
            uptime_secs: system::uptime_secs(),
            processes: if self.lists_processes {
                self.processes.sample(self.chip.memory_bytes, gpu_shares)
            } else {
                None
            },
        }
    }
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_of_this_machine_has_cores_memory_and_processes() {
        let mut sampler = Sampler::new();
        let snapshot = sampler.sample();
        assert!(
            !snapshot.cpu.cores.is_empty(),
            "the baseline was taken in new()"
        );
        assert!(snapshot.memory.is_some_and(|memory| memory.total > 0));
        assert!(snapshot.uptime_secs > 0);
        assert!(
            snapshot
                .processes
                .is_some_and(|processes| !processes.is_empty())
        );
        assert!(
            snapshot.network.is_some(),
            "the baseline was taken in new()"
        );
        sampler.set_lists_processes(false);
        assert!(sampler.sample().processes.is_none());
    }
}
