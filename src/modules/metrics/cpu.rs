//! Per-core load from /proc/stat; core tiers and clocks from sysfs.

use std::path::Path;

use crate::files;
use crate::snapshot::{CoreKind, CoreTier, CoreUsage, TierClock};

const CPUS: &str = "/sys/devices/system/cpu";

/// Per-core tick counters; usage is the busy share of the delta between reads.
#[derive(Default)]
pub struct CpuTicks {
    previous: Vec<Ticks>,
}

/// One core's ticks since boot, as /proc/stat sums them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ticks {
    busy: u64,
    total: u64,
}

impl CpuTicks {
    /// Busy shares, 0–100, in logical CPU order. Empty when the read fails,
    /// and when there is no baseline for these cores yet (the first call, or
    /// after the core count changes), which this read then becomes.
    pub fn sample(&mut self) -> Vec<f32> {
        let current = files::read("/proc/stat")
            .map(|stat| parse_ticks(&stat))
            .unwrap_or_default();
        self.sample_from(current)
    }

    fn sample_from(&mut self, current: Vec<Ticks>) -> Vec<f32> {
        // A failed read keeps the baseline, so the next sample spans the gap.
        if current.is_empty() {
            return Vec::new();
        }
        // Without a read of the same cores to measure against, this one only
        // becomes the baseline: since-boot counters are not an interval.
        let usage = if current.len() == self.previous.len() {
            self.previous
                .iter()
                .zip(&current)
                .map(|(&before, &now)| busy_share(before, now))
                .collect()
        } else {
            Vec::new()
        };
        self.previous = current;
        usage
    }
}

/// The `cpuN` lines of /proc/stat, in order. Busy time is user, nice, system,
/// irq, softirq and steal; idle time is idle and iowait. Guest time is
/// already inside user and nice. Steal counts as busy: on a VM, the cycles
/// the hypervisor gave to another guest were cycles this core couldn't use.
fn parse_ticks(stat: &str) -> Vec<Ticks> {
    stat.lines()
        .filter(|line| {
            line.strip_prefix("cpu")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
        .filter_map(|line| {
            let fields: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            if fields.len() < 4 {
                return None;
            }
            let sum = |indexes: &[usize]| {
                indexes
                    .iter()
                    .filter_map(|&ix| fields.get(ix))
                    .fold(0u64, |sum, &ticks| sum.saturating_add(ticks))
            };
            let busy = sum(&[0, 1, 2, 5, 6, 7]);
            Some(Ticks {
                busy,
                total: busy.saturating_add(sum(&[3, 4])),
            })
        })
        .collect()
}

/// Busy share, 0–100, between two reads of one core's counters. A counter
/// that went backwards (the kernel's don't) counts as no time passed.
fn busy_share(before: Ticks, now: Ticks) -> f32 {
    let busy = now.busy.saturating_sub(before.busy) as f64;
    let total = now.total.saturating_sub(before.total) as f64;
    if total > 0.0 {
        (busy / total * 100.0) as f32
    } else {
        0.0
    }
}

/// The chip's core tiers in logical CPU order. The kernel states each CPU's
/// speed as a `cpu_capacity` on arm64 (big.LITTLE), and where that's missing,
/// cpufreq's `cpuinfo_max_freq` tells Intel's P and E cores apart. Runs of
/// CPUs of one speed form a tier, kinds going by speed: Efficiency,
/// Performance, Super. A chip whose cores are all alike, as servers' are, has
/// one Performance tier, so its clock has a place in `Cpu::clocks`; a machine
/// that reports no CPUs has no tiers.
pub fn core_tiers() -> Vec<CoreTier> {
    let cpus = cpu_indexes(Path::new(CPUS));
    tiers_from(cpus.len(), speeds(Path::new(CPUS), &cpus).as_deref())
}

/// The logical CPUs the kernel knows, from its `cpuN` directories, in order.
fn cpu_indexes(cpus: &Path) -> Vec<u32> {
    let mut indexes: Vec<u32> = files::entries(cpus)
        .iter()
        .filter_map(|path| files::name(path)?.strip_prefix("cpu")?.parse().ok())
        .collect();
    indexes.sort_unstable();
    indexes
}

/// One speed class per CPU, from one source for all of them so the classes
/// compare: `cpu_capacity`, else `cpuinfo_max_freq`. `None` when neither
/// covers every CPU, as in most VMs.
fn speeds(cpus: &Path, indexes: &[u32]) -> Option<Vec<u64>> {
    let read = |file: &str| {
        indexes
            .iter()
            .map(|cpu| files::read_number(cpus.join(format!("cpu{cpu}/{file}"))))
            .collect::<Option<Vec<u64>>>()
    };
    read("cpu_capacity").or_else(|| read("cpufreq/cpuinfo_max_freq"))
}

/// Tiers from each CPU's speed class, in logical CPU order; `None` speeds
/// make one tier of all `count` CPUs.
fn tiers_from(count: usize, speeds: Option<&[u64]>) -> Vec<CoreTier> {
    if count == 0 {
        return Vec::new();
    }
    let Some(speeds) = speeds else {
        return vec![CoreTier {
            kind: CoreKind::Performance,
            cores: count as u32,
        }];
    };
    let mut classes = speeds.to_vec();
    classes.sort_unstable();
    classes.dedup();
    let kind = |speed: u64| {
        let rank = classes
            .iter()
            .position(|&class| class == speed)
            .unwrap_or(0);
        match (classes.len(), rank) {
            (1, _) => CoreKind::Performance,
            (_, 0) => CoreKind::Efficiency,
            (classes, rank) if classes > 2 && rank + 1 == classes => CoreKind::Super,
            _ => CoreKind::Performance,
        }
    };
    let mut tiers: Vec<CoreTier> = Vec::new();
    for kind in speeds.iter().map(|&speed| kind(speed)) {
        match tiers.last_mut() {
            Some(tier) if tier.kind == kind => tier.cores += 1,
            _ => tiers.push(CoreTier { kind, cores: 1 }),
        }
    }
    tiers
}

/// Labels logical CPUs by the tier each falls in, `tiers` being in logical
/// CPU order as [`core_tiers`] returns them. Cores no tier covers count as
/// Performance.
pub fn label_cores(usage: &[f32], tiers: &[CoreTier]) -> Vec<CoreUsage> {
    let kinds = tiers
        .iter()
        .flat_map(|tier| std::iter::repeat_n(tier.kind, tier.cores as usize))
        .chain(std::iter::repeat(CoreKind::Performance));
    usage
        .iter()
        .zip(kinds)
        .map(|(&usage, kind)| CoreUsage { kind, usage })
        .collect()
}

/// The clock of each tier in MHz: the mean of its cores' `scaling_cur_freq`
/// or, without cpufreq (VMs rarely have it), of the `cpu MHz` /proc/cpuinfo
/// prints for x86. Empty unless every tier's clock is known, so the list
/// lines up with `tiers`.
pub fn tier_clocks(tiers: &[CoreTier]) -> Vec<TierClock> {
    let count: usize = tiers.iter().map(|tier| tier.cores as usize).sum();
    let cpus = Path::new(CPUS);
    let mut mhz: Vec<Option<u32>> = cpu_indexes(cpus)
        .iter()
        .take(count)
        .map(|cpu| {
            files::read_number::<u64>(cpus.join(format!("cpu{cpu}/cpufreq/scaling_cur_freq")))
                .map(|khz| (khz / 1000) as u32)
        })
        .collect();
    if mhz.iter().any(Option::is_none)
        && let Some(cpuinfo) = files::read("/proc/cpuinfo")
    {
        mhz = cpuinfo_mhz(&cpuinfo);
    }
    clocks_from(tiers, &mhz)
}

/// The `cpu MHz` of each processor in /proc/cpuinfo, by processor number;
/// empty on ARM, whose cpuinfo has no clocks.
fn cpuinfo_mhz(cpuinfo: &str) -> Vec<Option<u32>> {
    let mut mhz = Vec::new();
    let mut processor = None;
    for line in cpuinfo.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "processor" => processor = value.trim().parse::<usize>().ok(),
            "cpu MHz" => {
                if let Some(ix) = processor {
                    if mhz.len() <= ix {
                        mhz.resize(ix + 1, None);
                    }
                    mhz[ix] = value
                        .trim()
                        .parse::<f64>()
                        .ok()
                        .map(|mhz| mhz.round() as u32);
                }
            }
            _ => {}
        }
    }
    mhz
}

/// One clock per tier, the mean over its cores, or none at all when a core's
/// clock is missing.
fn clocks_from(tiers: &[CoreTier], mhz: &[Option<u32>]) -> Vec<TierClock> {
    let mut at = 0;
    tiers
        .iter()
        .map(|tier| {
            let cores = mhz.get(at..at + tier.cores as usize)?;
            at += tier.cores as usize;
            let known: Vec<u64> = cores
                .iter()
                .map(|mhz| mhz.map(u64::from))
                .collect::<Option<_>>()?;
            let mean = known.iter().sum::<u64>().checked_div(known.len() as u64)?;
            Some(TierClock {
                kind: tier.kind,
                mhz: mean as u32,
            })
        })
        .collect::<Option<_>>()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str = "cpu  7915 0 2828 984037 756 0 555 185 0 0
cpu0 1981 0 677 246045 92 0 279 48 0 0
cpu1 2152 0 757 245711 281 0 153 35 0 0
intr 1361016 20 0 0
ctxt 2574893
btime 1791984757
";

    #[test]
    fn per_core_lines_are_read_and_the_total_is_skipped() {
        let ticks = parse_ticks(STAT);
        assert_eq!(ticks.len(), 2);
        // user + system + softirq + steal busy, idle + iowait idle.
        assert_eq!(
            ticks[0],
            Ticks {
                busy: 1981 + 677 + 279 + 48,
                total: 1981 + 677 + 279 + 48 + 246045 + 92
            }
        );
    }

    #[test]
    fn usage_is_the_busy_share_since_the_last_read() {
        let mut ticks = CpuTicks::default();
        let at = |busy, idle| {
            vec![Ticks {
                busy,
                total: busy + idle,
            }]
        };
        // Since-boot counters are not an interval, so there is nothing to report yet.
        assert_eq!(ticks.sample_from(at(100, 900)), Vec::<f32>::new());
        // A failed read keeps the baseline.
        assert_eq!(ticks.sample_from(Vec::new()), Vec::<f32>::new());
        // 10 busy ticks against 90 idle since the last good read.
        assert_eq!(ticks.sample_from(at(110, 990)), vec![10.0]);
    }

    #[test]
    fn a_changed_core_count_only_sets_a_new_baseline() {
        let mut ticks = CpuTicks::default();
        let core = Ticks { busy: 1, total: 2 };
        ticks.sample_from(vec![core]);
        assert_eq!(ticks.sample_from(vec![core, core]), Vec::<f32>::new());
    }

    #[test]
    fn alike_cores_form_one_performance_tier() {
        assert_eq!(tiers_from(0, None), Vec::<CoreTier>::new());
        let tiers = tiers_from(4, Some(&[1024, 1024, 1024, 1024]));
        assert_eq!(tiers.len(), 1);
        assert_eq!((tiers[0].kind, tiers[0].cores), (CoreKind::Performance, 4));
        assert_eq!(tiers_from(4, None), tiers);
    }

    #[test]
    fn speed_classes_form_tiers_in_cpu_order() {
        // Little cores first, as most big.LITTLE boards number them.
        let tiers = tiers_from(6, Some(&[400, 400, 1024, 1024, 1024, 1024]));
        let kinds: Vec<_> = tiers.iter().map(|t| (t.kind, t.cores)).collect();
        assert_eq!(
            kinds,
            [(CoreKind::Efficiency, 2), (CoreKind::Performance, 4)]
        );
        // Three classes: the fastest is Super. Big cores first here.
        let tiers = tiers_from(4, Some(&[1024, 1024, 500, 2000]));
        let kinds: Vec<_> = tiers.iter().map(|t| (t.kind, t.cores)).collect();
        assert_eq!(
            kinds,
            [
                (CoreKind::Performance, 2),
                (CoreKind::Efficiency, 1),
                (CoreKind::Super, 1)
            ]
        );
    }

    #[test]
    fn cores_take_their_tier_in_order_and_the_rest_count_as_performance() {
        let tiers = [
            CoreTier {
                kind: CoreKind::Efficiency,
                cores: 2,
            },
            CoreTier {
                kind: CoreKind::Super,
                cores: 1,
            },
        ];
        let kinds: Vec<CoreKind> = label_cores(&[1.0, 2.0, 3.0, 4.0], &tiers)
            .iter()
            .map(|core| core.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                CoreKind::Efficiency,
                CoreKind::Efficiency,
                CoreKind::Super,
                CoreKind::Performance
            ]
        );
        assert!(
            label_cores(&[1.0], &[])
                .iter()
                .all(|core| core.kind == CoreKind::Performance)
        );
    }

    #[test]
    fn clocks_are_per_tier_means_or_nothing() {
        let tiers = [
            CoreTier {
                kind: CoreKind::Efficiency,
                cores: 2,
            },
            CoreTier {
                kind: CoreKind::Performance,
                cores: 1,
            },
        ];
        let clocks = clocks_from(&tiers, &[Some(2000), Some(2100), Some(3500)]);
        let mhz: Vec<_> = clocks.iter().map(|c| (c.kind, c.mhz)).collect();
        assert_eq!(
            mhz,
            [(CoreKind::Efficiency, 2050), (CoreKind::Performance, 3500)]
        );
        // One unknown core leaves the list empty rather than misaligned.
        assert!(clocks_from(&tiers, &[Some(2000), None, Some(3500)]).is_empty());
        assert!(clocks_from(&tiers, &[Some(2000)]).is_empty());
    }

    #[test]
    fn cpuinfo_clocks_are_indexed_by_processor() {
        let cpuinfo =
            "processor\t: 1\ncpu MHz\t\t: 2700.000\n\nprocessor\t: 0\ncpu MHz\t\t: 2699.600\n";
        assert_eq!(cpuinfo_mhz(cpuinfo), [Some(2700), Some(2700)]);
        assert!(cpuinfo_mhz("CPU implementer\t: 0x41\n").is_empty());
    }

    #[test]
    fn this_machine_reports_its_cores() {
        let tiers = core_tiers();
        assert!(!tiers.is_empty());
        let mut ticks = CpuTicks::default();
        assert_eq!(ticks.sample().len(), 0, "the first read is the baseline");
        let cores: u32 = tiers.iter().map(|t| t.cores).sum();
        assert_eq!(ticks.sample().len(), cores as usize);
    }
}
