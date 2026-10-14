/// Static facts about the machine, read once.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct ChipInfo {
    /// Marketing name, for example `Apple M1 Max`.
    pub name: String,
    /// In logical CPU order, as [`core_tiers`](super::cpu::core_tiers) reads
    /// them; empty when macOS reports no performance levels.
    pub tiers: Vec<CoreTier>,
    pub gpu_cores: u32,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CoreKind {
    Efficiency,
    Performance,
    /// The tier above Performance, as on the M5 Pro.
    Super,
}

/// One performance level of the chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CoreTier {
    pub kind: CoreKind,
    /// Logical CPUs in this level.
    pub cores: u32,
}

#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct CoreUsage {
    pub kind: CoreKind,
    /// Busy share of the interval, 0–100.
    pub usage: f32,
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Cpu {
    pub cores: Vec<CoreUsage>,
    /// One per core tier, in [`ChipInfo::tiers`] order, since two tiers can
    /// share a kind; empty without IOReport.
    pub clocks: Vec<TierClock>,
}

/// The clock of one core tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[non_exhaustive]
pub struct Gpu {
    /// Active residency, 0–100.
    pub usage: f32,
    pub mhz: u32,
}

/// Power draw in watts, averaged over the sample interval. A block is `None`
/// without IOReport, and CPU, ANE and DRAM also while their energy counters
/// are dead, as macOS 27 betas leave them.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Power {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub ane: Option<f32>,
    pub dram: Option<f32>,
    /// Whole-system draw as the SMC reports it; `None` where it doesn't.
    pub system: Option<f32>,
}

impl Power {
    /// The SMC's whole-system draw when it reports one, otherwise the sum of
    /// the CPU, GPU, ANE and DRAM blocks, which leaves out the rest of the
    /// machine. `None` when a block is missing, so a dead counter can't
    /// shrink that sum unnoticed.
    pub fn total(&self) -> Option<f32> {
        self.system
            .or_else(|| Some(self.cpu? + self.gpu? + self.ane? + self.dram?))
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Temperatures {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ThermalPressure {
    Nominal,
    Moderate,
    Heavy,
    Trapping,
    Sleeping,
}

impl ThermalPressure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Nominal => "Nominal",
            Self::Moderate => "Moderate",
            Self::Heavy => "Heavy",
            Self::Trapping => "Trapping",
            Self::Sleeping => "Sleeping",
        }
    }
}

/// From the `com.apple.system.thermalpressurelevel` notify state, whose
/// levels <libkern/OSThermalNotification.h> numbers 0–4 on macOS.
impl TryFrom<u64> for ThermalPressure {
    type Error = u64;

    fn try_from(state: u64) -> Result<Self, Self::Error> {
        match state {
            0 => Ok(Self::Nominal),
            1 => Ok(Self::Moderate),
            2 => Ok(Self::Heavy),
            3 => Ok(Self::Trapping),
            4 => Ok(Self::Sleeping),
            unknown => Err(unknown),
        }
    }
}

/// In bytes.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Memory {
    pub used: u64,
    pub total: u64,
    /// `None` when `vm.swapusage` can't be read.
    pub swap: Option<Swap>,
}

/// In bytes.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Swap {
    pub used: u64,
    pub total: u64,
}

/// Byte rates per second.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Throughput {
    pub read: f64,
    pub write: f64,
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Volume {
    pub name: String,
    pub mount_point: String,
    pub total: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Battery {
    pub percent: u32,
    pub on_ac: bool,
}

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Process {
    pub pid: i32,
    /// Start time in microseconds since the epoch; with `pid`, the process identity.
    pub started_at: u64,
    pub user: String,
    pub command: String,
    /// Share of one core, so a busy multi-threaded process can exceed 100.
    pub cpu: f32,
    /// Share of the GPU's busy time, 0–100.
    pub gpu: f32,
    /// Resident memory as a share of physical memory, 0–100.
    pub memory: f32,
    pub resident: u64,
    /// Total CPU time consumed, in seconds.
    pub cpu_time: f64,
}

/// Everything measured over one sample interval.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Snapshot {
    pub cpu: Cpu,
    /// `None` without IOReport.
    pub gpu: Option<Gpu>,
    /// Neural Engine load, 0–100, estimated from its power draw: Apple
    /// exposes no occupancy counter. `None` without a power reading.
    pub ane: Option<f32>,
    /// Bytes per second the Neural Engine moved through memory; `None` where
    /// the chip has no such counter.
    pub ane_traffic: Option<f64>,
    pub power: Power,
    pub temperatures: Temperatures,
    /// `None` when the notify state can't be read or is a level macOS
    /// doesn't define.
    pub thermal: Option<ThermalPressure>,
    /// `None` when the kernel's page counts can't be read.
    pub memory: Option<Memory>,
    /// `read` is received, `write` is sent. `None` on the first sample, which
    /// only sets the baseline, and when the interface counters can't be read.
    pub network: Option<Throughput>,
    /// `None` on the first sample, which only sets the baseline, and when no
    /// physical disk's counters can be read.
    pub disk: Option<Throughput>,
    pub volumes: Vec<Volume>,
    pub battery: Option<Battery>,
    pub uptime_secs: u64,
    pub processes: Vec<Process>,
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

    /// CPU, GPU, ANE and DRAM draw, in that order.
    const BLOCKS: [Option<f32>; 4] = [Some(4.0), Some(2.0), Some(0.5), Some(1.0)];

    fn power_without_smc([cpu, gpu, ane, dram]: [Option<f32>; 4]) -> Power {
        Power {
            cpu,
            gpu,
            ane,
            dram,
            system: None,
        }
    }

    #[test]
    fn power_total_is_the_smc_reading_when_there_is_one() {
        let power = Power {
            system: Some(30.0),
            ..power_without_smc(BLOCKS)
        };
        assert_eq!(power.total(), Some(30.0));
    }

    #[test]
    fn power_total_without_smc_is_the_sum_of_all_four_blocks() {
        assert_eq!(power_without_smc(BLOCKS).total(), Some(7.5));
    }

    #[test]
    fn power_total_without_smc_and_with_a_dead_counter_is_no_reading() {
        // macOS 27 betas leave the CPU, ANE and DRAM energy counters dead.
        for dead in 0..BLOCKS.len() {
            let mut blocks = BLOCKS;
            blocks[dead] = None;
            assert_eq!(power_without_smc(blocks).total(), None, "block {dead} dead");
        }
    }

    #[test]
    fn thermal_pressure_maps_the_macos_notify_levels() {
        // `OSThermalPressureLevel` in <libkern/OSThermalNotification.h>, macOS branch.
        let levels = [
            (0, ThermalPressure::Nominal),
            (1, ThermalPressure::Moderate),
            (2, ThermalPressure::Heavy),
            (3, ThermalPressure::Trapping),
            (4, ThermalPressure::Sleeping),
        ];
        for (state, level) in levels {
            assert_eq!(ThermalPressure::try_from(state), Ok(level), "state {state}");
        }
    }

    #[test]
    fn unknown_thermal_level_is_rejected_rather_than_read_as_nominal() {
        // 10 is `Light` in the iOS numbering, which macOS doesn't use.
        for state in [5, 10, u64::MAX] {
            assert_eq!(ThermalPressure::try_from(state), Err(state));
        }
    }
}