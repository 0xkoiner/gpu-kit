/// Static facts about the machine, read once.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct ChipInfo {
    /// The CPU's model name, for example `Intel(R) Xeon(R) Platinum 8259CL CPU
    /// @ 2.50GHz`, or the core an ARM chip is built on.
    pub name: String,
    /// In logical CPU order, a tier being a run of cores of one speed; a chip
    /// whose cores are all alike has one. Empty when no CPU is reported.
    pub tiers: Vec<CoreTier>,
    pub gpu_cores: u32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub enum CoreKind {
    Efficiency,
    Performance,
    /// The tier above Performance, where a chip has three.
    Super,
}

/// One performance level of the chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct CoreTier {
    pub kind: CoreKind,
    /// Logical CPUs in this level.
    pub cores: u32,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct CoreUsage {
    pub kind: CoreKind,
    /// Busy share of the interval, 0–100.
    pub usage: f32,
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Cpu {
    pub cores: Vec<CoreUsage>,
    /// One per core tier, in [`ChipInfo::tiers`] order, since two tiers can
    /// share a kind; empty without cpufreq when /proc/cpuinfo has no clocks
    /// either.
    pub clocks: Vec<TierClock>,
}

/// The clock of one core tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct TierClock {
    pub kind: CoreKind,
    pub mhz: u32,
}

impl Cpu {
    /// Mean busy share across all cores, 0–100. `None` without core samples,
    /// so a missing reading is not shown as an idle CPU.
    pub fn usage(&self) -> Option<f32> {
        if self.cores.is_empty() {
            return None;
        }
        let total: f32 = self.cores.iter().map(|core| core.usage).sum();
        Some(total / self.cores.len() as f32)
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Gpu {
    /// Busy share, 0–100.
    pub usage: f32,
    /// `None` where the driver reports no clock.
    pub mhz: Option<u32>,
}

/// Power draw in watts, averaged over the sample interval. CPU and DRAM come
/// from RAPL, which VMs rarely expose and recent kernels open to root alone;
/// GPU from its driver. A block is `None` where it can't be read.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Power {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub dram: Option<f32>,
    /// Whole-system draw, where the machine meters it: RAPL's `psys` domain
    /// or, on battery, the battery's discharge. `None` elsewhere.
    pub system: Option<f32>,
}

impl Power {
    /// The metered whole-system draw when there is one, otherwise the sum of
    /// the CPU, GPU and DRAM blocks, which leaves out the rest of the machine.
    /// `None` when a block is missing, so an unread counter can't shrink that
    /// sum unnoticed.
    pub fn total(&self) -> Option<f32> {
        self.system
            .or_else(|| Some(self.cpu? + self.gpu? + self.dram?))
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Temperatures {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
}

/// How hard the kernel is holding temperatures down, read off its thermal
/// zones' trip points: Moderate past a `passive` trip, Heavy past `hot`,
/// Trapping past `critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub enum ThermalPressure {
    Nominal,
    Moderate,
    Heavy,
    Trapping,
}

impl ThermalPressure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nominal => "Nominal",
            Self::Moderate => "Moderate",
            Self::Heavy => "Heavy",
            Self::Trapping => "Trapping",
        }
    }
}

/// In bytes.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Memory {
    pub used: u64,
    pub total: u64,
    /// `None` when /proc/meminfo has no swap figures.
    pub swap: Option<Swap>,
}

/// In bytes.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Swap {
    pub used: u64,
    pub total: u64,
}

/// Byte rates per second.
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Throughput {
    pub read: f64,
    pub write: f64,
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Volume {
    pub name: String,
    pub mount_point: String,
    pub total: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Battery {
    pub percent: u32,
    pub on_ac: bool,
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Process {
    pub pid: i32,
    /// Start time in microseconds since the epoch; with `pid`, the process identity.
    pub started_at: u64,
    pub user: String,
    pub command: String,
    /// Share of one core, so a busy multi-threaded process can exceed 100.
    pub cpu: f32,
    /// Share of the GPU's busy time, 0–100. `Some` only where a driver
    /// reports per-process GPU time (NVML); `None` elsewhere.
    pub gpu: Option<f32>,
    /// Resident memory as a share of physical memory, 0–100.
    pub memory: f32,
    pub resident: u64,
    /// Total CPU time consumed, in seconds.
    pub cpu_time: f64,
}

/// Everything measured over one sample interval.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[non_exhaustive]
pub struct Snapshot {
    pub cpu: Cpu,
    /// `None` without a GPU driver's telemetry (NVML or amdgpu).
    pub gpu: Option<Gpu>,
    pub power: Power,
    pub temperatures: Temperatures,
    /// `None` without thermal zones, as in a VM.
    pub thermal: Option<ThermalPressure>,
    /// `None` when /proc/meminfo can't be read.
    pub memory: Option<Memory>,
    /// `read` is received, `write` is sent. `None` on the first sample, which
    /// only sets the baseline, when the interface counters can't be read, and
    /// when no interface carries its own traffic.
    pub network: Option<Throughput>,
    /// `None` on the first sample, which only sets the baseline, and when no
    /// physical disk's counters can be read.
    pub disk: Option<Throughput>,
    /// `None` when the mount table can't be read.
    pub volumes: Option<Vec<Volume>>,
    pub battery: Option<Battery>,
    pub uptime_secs: u64,
    /// `None` while [`Sampler`](crate::Sampler) isn't listing processes, and
    /// when the process table can't be read.
    pub processes: Option<Vec<Process>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_with_shares(shares: &[f32]) -> Cpu {
        Cpu {
            cores: shares
                .iter()
                .map(|&usage| CoreUsage {
                    kind: CoreKind::Performance,
                    usage,
                })
                .collect(),
            ..Cpu::default()
        }
    }

    #[test]
    fn cpu_usage_is_the_mean_of_the_core_shares() {
        assert_eq!(cpu_with_shares(&[10.0, 30.0, 80.0]).usage(), Some(40.0));
    }

    #[test]
    fn cpu_usage_without_core_samples_is_no_reading_rather_than_idle() {
        // `CpuTicks::sample` returns no shares on its first call and on a failed read.
        assert_eq!(cpu_with_shares(&[]).usage(), None);
    }

    /// CPU, GPU and DRAM draw, in that order.
    const BLOCKS: [Option<f32>; 3] = [Some(4.0), Some(2.0), Some(1.0)];

    fn power_without_meter([cpu, gpu, dram]: [Option<f32>; 3]) -> Power {
        Power {
            cpu,
            gpu,
            dram,
            system: None,
        }
    }

    #[test]
    fn power_total_is_the_metered_draw_when_there_is_one() {
        let power = Power {
            system: Some(30.0),
            ..power_without_meter(BLOCKS)
        };
        assert_eq!(power.total(), Some(30.0));
    }

    #[test]
    fn power_total_without_a_meter_is_the_sum_of_all_three_blocks() {
        assert_eq!(power_without_meter(BLOCKS).total(), Some(7.0));
    }

    #[test]
    fn power_total_without_a_meter_and_with_an_unread_block_is_no_reading() {
        // RAPL may be closed to us, or the GPU driver absent.
        for dead in 0..BLOCKS.len() {
            let mut blocks = BLOCKS;
            blocks[dead] = None;
            assert_eq!(
                power_without_meter(blocks).total(),
                None,
                "block {dead} unread"
            );
        }
    }
}
