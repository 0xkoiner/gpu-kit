//! Power, DVFS residency and Neural Engine traffic from the private
//! `libIOReport`, as mactop reads them.
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::ptr;
use std::time::Instant;

use super::cf::{
    self, CFDictionaryRef, CFMutableDictionaryRef, CFOwned, CFStringRef, CFType, CFTypeRef,
    io_object_t,
};
use crate::snapshot::{CoreTier, TierClock};

type IOReportSubscriptionRef = *const c_void;

#[link(name = "IOReport")]
unsafe extern "C" {
    fn IOReportCopyChannelsInGroup(
        group: CFStringRef,
        subgroup: CFStringRef,
        a: u64,
        b: u64,
        c: u64,
    ) -> CFDictionaryRef;
    fn IOReportMergeChannels(a: CFDictionaryRef, b: CFDictionaryRef, nil: CFTypeRef);
    fn IOReportCreateSubscription(
        allocator: *const c_void,
        channels: CFMutableDictionaryRef,
        subscribed: *mut CFMutableDictionaryRef,
        id: u64,
        nil: CFTypeRef,
    ) -> IOReportSubscriptionRef;
    fn IOReportCreateSamples(
        subscription: IOReportSubscriptionRef,
        channels: CFMutableDictionaryRef,
        nil: CFTypeRef,
    ) -> CFDictionaryRef;
    fn IOReportCreateSamplesDelta(
        previous: CFDictionaryRef,
        current: CFDictionaryRef,
        nil: CFTypeRef,
    ) -> CFDictionaryRef;
    fn IOReportChannelGetGroup(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetSubGroup(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetChannelName(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetUnitLabel(item: CFDictionaryRef) -> CFStringRef;
    fn IOReportSimpleGetIntegerValue(item: CFDictionaryRef, index: i32) -> i64;
    fn IOReportStateGetCount(item: CFDictionaryRef) -> i32;
    fn IOReportStateGetNameForIndex(item: CFDictionaryRef, index: i32) -> CFStringRef;
    fn IOReportStateGetResidency(item: CFDictionaryRef, index: i32) -> i64;
}

/// Watts per block, averaged over the interval between two samples. `None`
/// for a block with no channel on this chip and OS, and for one whose counter
/// can't be read this interval: dead, gone backwards or in an unknown unit.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Energy {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub gpu_sram: Option<f32>,
    pub ane: Option<f32>,
    pub dram: Option<f32>,
}

/// One block's energy over an interval, summed over its channels.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum Joules {
    /// No channel for the block on this chip and OS.
    #[default]
    Missing,
    Counted(f64),
    /// A channel couldn't be read this interval.
    Unreadable,
}

impl Joules {
    /// Adds a channel's count. `None` (an unknown unit), a negative count (a
    /// counter reset, or IOReport's invalid-value sentinel) or NaN leaves the
    /// block with no reading.
    fn plus(self, joules: Option<f64>) -> Self {
        match (self, joules.filter(|joules| *joules >= 0.0)) {
            (Self::Missing, Some(joules)) => Self::Counted(joules),
            (Self::Counted(sum), Some(joules)) => Self::Counted(sum + joules),
            _ => Self::Unreadable,
        }
    }

    fn counted(self) -> Option<f64> {
        match self {
            Self::Counted(joules) => Some(joules),
            _ => None,
        }
    }
}

/// Each block's energy over one interval, as `Energy Model` channels count it.
#[derive(Debug, Default)]
struct EnergyCounts {
    cpu: Joules,
    gpu: Joules,
    gpu_sram: Joules,
    ane: Joules,
    dram: Joules,
}

impl EnergyCounts {
    /// Adds a channel's joules to its block. Exact names only: sub-component
    /// channels would double count.
    fn add(&mut self, name: &str, joules: Option<f64>) {
        let block = if name.contains("CPU Energy") {
            &mut self.cpu
        } else if name == "GPU Energy" {
            &mut self.gpu
        } else if name.starts_with("GPU SRAM") {
            &mut self.gpu_sram
        } else if name.starts_with("ANE") {
            &mut self.ane
        } else if name.starts_with("DRAM") {
            &mut self.dram
        } else {
            return;
        };
        *block = block.plus(joules);
    }

    /// Average watts over `secs`. A counter that didn't move is dead, as
    /// macOS 27 betas freeze them, when its block can't idle at 0 J: the CPU
    /// and DRAM can't over an interval this code ran in. An idle Neural Engine
    /// draws nothing, so its zero only counts as dead beside a dead CPU counter.
    fn watts(&self, secs: f64) -> Energy {
        let cpu = self.cpu.counted();
        let cpu_dead = cpu == Some(0.0);
        let moved = |joules: Option<f64>| joules.filter(|&joules| joules > 0.0);
        let watts = |joules: Option<f64>| joules.map(|joules| (joules / secs) as f32);
        Energy {
            cpu: watts(moved(cpu)),
            gpu: watts(self.gpu.counted()),
            gpu_sram: watts(self.gpu_sram.counted()),
            ane: watts(
                self.ane
                    .counted()
                    .filter(|&joules| joules > 0.0 || !cpu_dead),
            ),
            dram: watts(moved(self.dram.counted())),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reading {
    pub energy: Energy,
    /// Bytes per second the Neural Engine moved through memory; `None`
    /// where the chip has no such counter.
    pub ane_traffic: Option<f64>,
    /// `None` without a GPU performance-state channel.
    pub gpu: Option<GpuReading>,
    /// The clock of each CPU cluster that has core channels, slowest first:
    /// `ECPU`, `MCPU` (the Performance cores of the M5 Pro and later), `PCPU`.
    /// `None` for a cluster whose frequency table wasn't found.
    pub cpu_mhz: Vec<Option<u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpuReading {
    /// Active residency, 0–100.
    pub usage: f32,
    /// `None` without the GPU's frequency table.
    pub mhz: Option<u32>,
}

impl Reading {
    /// The clocks of `tiers`, as [`Cpu::clocks`](super::snapshot::Cpu::clocks)
    /// holds them. Clusters pair with tiers in order, both being slowest
    /// first: ECPU and PCPU are Efficiency and Performance on M1–M4, and
    /// Efficiency and Super on the M5; MCPU and PCPU are Performance and
    /// Super on the M5 Pro and Max. Empty unless every tier's clock is known.
    pub fn tier_clocks(&self, tiers: &[CoreTier]) -> Vec<TierClock> {
        if self.cpu_mhz.len() != tiers.len() {
            return Vec::new();
        }
        tiers
            .iter()
            .zip(&self.cpu_mhz)
            .map(|(tier, &mhz)| {
                Some(TierClock {
                    kind: tier.kind,
                    mhz: mhz?,
                })
            })
            .collect::<Option<_>>()
            .unwrap_or_default()
    }
}

/// DVFS tables in MHz, from the power manager's `voltage-states*` properties.
#[derive(Debug, Default)]
struct FrequencyTables {
    /// Indexed as [`cpu_cluster`] numbers the clusters.
    cpu: [Vec<u32>; 3],
    gpu: Vec<u32>,
}

pub struct IOReport {
    subscription: CFOwned,
    channels: CFOwned,
    /// Released on drop; samples read `channels`.
    _subscribed: Option<CFOwned>,
    previous: Option<(Instant, CFOwned)>,
    tables: FrequencyTables,
}

// The subscription and dictionaries are only touched by the thread that owns the sampler.
unsafe impl Send for IOReport {}

impl IOReport {
    /// Subscribes to energy, GPU and CPU performance-state and memory traffic
    /// channels, whichever this chip and OS have; `None` when IOReport has
    /// none of them or is unavailable (for example inside an app sandbox).
    pub fn new() -> Option<Self> {
        let groups = [
            ("Energy Model", None),
            ("AMC Stats", Some("Perf Counters")),
            ("GPU Stats", Some("GPU Performance States")),
            ("CPU Stats", Some("CPU Core Performance States")),
        ];
        let mut found = groups
            .into_iter()
            .filter_map(|(group, subgroup)| copy_channels(group, subgroup));
        let merged = found.next()?;
        for channels in found {
            // The copied channels are a mutable dictionary, which this merges into.
            unsafe { IOReportMergeChannels(merged.as_ptr(), channels.as_ptr(), ptr::null()) };
        }
        unsafe {
            let count = cf::CFDictionaryGetCount(merged.as_ptr());
            let channels = CFOwned::new(cf::CFDictionaryCreateMutableCopy(
                cf::kCFAllocatorDefault,
                count,
                merged.as_ptr(),
            ))?;
            let mut subscribed: CFMutableDictionaryRef = ptr::null_mut();
            let subscription = IOReportCreateSubscription(
                ptr::null(),
                channels.as_ptr().cast_mut(),
                &mut subscribed,
                0,
                ptr::null(),
            );
            // Owned before the check, so a failed call that filled it in doesn't leak it.
            let subscribed = CFOwned::new(subscribed);
            Some(Self {
                subscription: CFOwned::new(subscription)?,
                channels,
                _subscribed: subscribed,
                previous: None,
                tables: FrequencyTables::read(),
            })
        }
    }

    /// Reads the channels accumulated since the previous call; the first call
    /// only records a baseline.
    pub fn sample(&mut self) -> Option<Reading> {
        let now = Instant::now();
        // `Create` calls hand back +1 references.
        let current = unsafe {
            CFOwned::new(IOReportCreateSamples(
                self.subscription.as_ptr(),
                self.channels.as_ptr().cast_mut(),
                ptr::null(),
            ))
        }?;
        let (at, previous) = self.previous.replace((now, current))?;
        let current = &self.previous.as_ref()?.1;
        let delta = unsafe {
            CFOwned::new(IOReportCreateSamplesDelta(
                previous.as_ptr(),
                current.as_ptr(),
                ptr::null(),
            ))
        }?;
        let secs = now.duration_since(at).as_secs_f64().max(1e-3);
        Some(self.read_delta(&delta, secs))
    }

    fn read_delta(&self, delta: &CFType, secs: f64) -> Reading {
        let mut energy = EnergyCounts::default();
        let mut ane_bytes: Option<i64> = None;
        let mut gpu = Dvfs::default();
        let mut cpu: [Dvfs; 3] = Default::default();
        let channels = cf::dict_get(delta, "IOReportChannels");
        for item in channels.into_iter().flat_map(cf::array_items) {
            let channel = item.as_ptr();
            // The channel owns the strings these `Get` calls return.
            let (group, subgroup, name) = unsafe {
                (
                    borrowed_string(IOReportChannelGetGroup(channel)),
                    borrowed_string(IOReportChannelGetSubGroup(channel)),
                    borrowed_string(IOReportChannelGetChannelName(channel)),
                )
            };
            let unit = || unsafe { borrowed_string(IOReportChannelGetUnitLabel(channel)) };
            match (group.as_str(), subgroup.as_str()) {
                ("Energy Model", _) => {
                    let count = unsafe { IOReportSimpleGetIntegerValue(channel, 0) } as f64;
                    let divisor = unit_divisor(unit().trim());
                    energy.add(&name, divisor.map(|divisor| count / divisor));
                }
                ("AMC Stats", _) => {
                    if is_ane_traffic(&name, unit().trim()) {
                        let bytes = unsafe { IOReportSimpleGetIntegerValue(channel, 0) };
                        ane_bytes = Some(ane_bytes.unwrap_or(0).saturating_add(bytes.max(0)));
                    }
                }
                ("GPU Stats", "GPU Performance States") if name == "GPUPH" => {
                    gpu.add(&states(item), &self.tables.gpu);
                }
                ("CPU Stats", "CPU Core Performance States") => {
                    if let Some(cluster) = cpu_cluster(&name) {
                        cpu[cluster].add(&states(item), &self.tables.cpu[cluster]);
                    }
                }
                _ => {}
            }
        }
        Reading {
            energy: energy.watts(secs),
            ane_traffic: ane_bytes.map(|bytes| bytes as f64 / secs),
            gpu: gpu.usage().map(|usage| GpuReading {
                usage,
                mhz: gpu.mhz(&self.tables.gpu),
            }),
            cpu_mhz: cpu
                .iter()
                .zip(&self.tables.cpu)
                .filter(|(cluster, _)| cluster.channels > 0)
                .map(|(cluster, freqs)| cluster.mhz(freqs))
                .collect(),
        }
    }
}

fn copy_channels(group: &str, subgroup: Option<&str>) -> Option<CFOwned> {
    let group = CFOwned::string(group);
    let subgroup = subgroup.map(CFOwned::string);
    let subgroup_ptr = subgroup.as_ref().map_or(ptr::null(), |s| s.as_ptr());
    // A `Copy` call hands back a +1 reference.
    unsafe {
        CFOwned::new(IOReportCopyChannelsInGroup(
            group.as_ptr(),
            subgroup_ptr,
            0,
            0,
            0,
        ))
    }
}

/// Reads a string an IOReport `Get` call returned.
///
/// # Safety
/// `cf` must be null or a CF object that stays alive for the call.
unsafe fn borrowed_string(cf: CFStringRef) -> String {
    unsafe { CFType::borrowed(cf) }
        .and_then(cf::string)
        .unwrap_or_default()
}

/// The Neural Engine's own reads and writes in `AMC Stats`, such as
/// `ANE0 RD`. The `DCS` channels see the same traffic at the DRAM scheduler,
/// and `PMP` repeats these names in `events` that read `INT64_MIN`, so the
/// group, name and unit must all match.
fn is_ane_traffic(name: &str, unit: &str) -> bool {
    unit == "B"
        && name.starts_with("ANE")
        && !name.contains("DCS")
        && (name.ends_with(" RD") || name.ends_with(" WR"))
}

/// Units per joule; `None` for a unit this doesn't know, rather than a guess
/// that could be off by orders of magnitude.
fn unit_divisor(unit: &str) -> Option<f64> {
    match unit {
        "J" => Some(1.0),
        "mJ" => Some(1e3),
        "uJ" => Some(1e6),
        "nJ" => Some(1e9),
        "pJ" => Some(1e12),
        _ => None,
    }
}

/// `(state name, residency)` pairs of a state channel. A negative residency,
/// IOReport's invalid-value sentinel, reads as none.
fn states(item: &CFType) -> Vec<(String, i64)> {
    let item = item.as_ptr();
    let count = unsafe { IOReportStateGetCount(item) };
    (0..count)
        .map(|ix| unsafe {
            // The channel owns the state names.
            let name = borrowed_string(IOReportStateGetNameForIndex(item, ix));
            (name, IOReportStateGetResidency(item, ix).max(0))
        })
        .collect()
}

fn is_inactive(state: &str) -> bool {
    matches!(state, "OFF" | "IDLE" | "DOWN")
}

/// The cluster of a `CPU Core Performance States` channel, numbered slowest
/// first: 0 for `ECPU` cores, 1 for `MCPU` and 2 for `PCPU`. The tag can sit
/// anywhere in the name, as in `PACC0_MCPU2` or `DIE_1_PCPU1_CPU0`.
fn cpu_cluster(channel: &str) -> Option<usize> {
    ["ECPU", "MCPU", "PCPU"]
        .iter()
        .position(|cluster| channel.contains(cluster))
}

/// Busy share and residency-weighted clock of a group of DVFS state channels:
/// the cores of one CPU cluster, or the GPU's dies.
#[derive(Debug, Default)]
struct Dvfs {
    channels: u32,
    /// Residency of every state, and of the active ones, over all channels.
    total: f64,
    active: f64,
    /// Sum of the clocks of the channels that ran at a known frequency.
    mhz_sum: f64,
    clocked: u32,
}

impl Dvfs {
    /// Adds a channel whose active states run at `freqs`, in order. Active
    /// states past the table's end run at a frequency this doesn't know.
    fn add(&mut self, states: &[(String, i64)], freqs: &[u32]) {
        self.channels += 1;
        let mut known = 0f64;
        let mut weighted = 0f64;
        let active = states.iter().filter(|(name, _)| !is_inactive(name));
        for (k, (_, residency)) in active.enumerate() {
            let residency = *residency as f64;
            self.active += residency;
            if let Some(&mhz) = freqs.get(k) {
                known += residency;
                weighted += mhz as f64 * residency;
            }
        }
        self.total += states.iter().map(|(_, r)| *r as f64).sum::<f64>();
        if known > 0.0 {
            self.mhz_sum += weighted / known;
            self.clocked += 1;
        }
    }

    /// Busy share, 0–100; `None` without residency to measure it by.
    fn usage(&self) -> Option<f32> {
        (self.total > 0.0).then(|| (self.active / self.total * 100.0) as f32)
    }

    /// Mean clock of the channels while active. `None` without channels or a
    /// table, and when they were busy only in states past the table's end;
    /// the table's lowest when every channel idled.
    fn mhz(&self, freqs: &[u32]) -> Option<u32> {
        let floor = *freqs.first()?;
        if self.channels == 0 {
            return None;
        }
        if self.clocked == 0 {
            return (self.active == 0.0).then_some(floor);
        }
        Some(((self.mhz_sum / self.clocked as f64) as u32).max(floor))
    }
}

impl FrequencyTables {
    /// Reads the tables of the `pmgr` node, or of `pmgr-child` on M6 and
    /// later, where `pmgr` is a stub. The first node with a table wins, so a
    /// stub can't clobber it.
    fn read() -> Self {
        let mut tables = Self::default();
        for node in ["pmgr", "pmgr-child"] {
            cf::for_each_named_service(node, |entry| tables.fill_from(entry));
        }
        tables
    }

    /// Fills the tables still empty from `entry`'s properties.
    fn fill_from(&mut self, entry: io_object_t) {
        // The table under the first of `keys` that has one.
        let first_table = |keys: &[Option<&str>]| {
            keys.iter()
                .flatten()
                .map(|key| voltage_states(entry, key))
                .find(|freqs| !freqs.is_empty())
                .unwrap_or_default()
        };
        // M5 and later name each cluster's table in `acc-clusters`; M1–M4 use
        // fixed keys for their ECPU and PCPU tables, and have no MCPU.
        let listed = cf::property(entry, "acc-clusters")
            .and_then(|data| cf::data_bytes(&data).map(cluster_tables))
            .unwrap_or_default();
        let fixed = [
            Some("voltage-states1-sram"),
            None,
            Some("voltage-states5-sram"),
        ];
        for ((freqs, listed), fixed) in self.cpu.iter_mut().zip(&listed).zip(fixed) {
            if freqs.is_empty() {
                *freqs = first_table(&[listed.as_deref(), fixed]);
            }
        }
        if self.gpu.is_empty() {
            self.gpu = first_table(&[Some("voltage-states9-sram"), Some("voltage-states9")]);
        }
    }
}

/// The DVFS table under `key` of a power manager node; empty without one.
fn voltage_states(entry: io_object_t, key: &str) -> Vec<u32> {
    cf::property(entry, key)
        .and_then(|data| cf::data_bytes(&data).map(parse_voltage_states))
        .unwrap_or_default()
}

/// The `voltage-states` key of each CPU cluster, indexed as [`cpu_cluster`]
/// numbers them, from `acc-clusters`: 8-byte entries that start with the
/// table's number and the cluster's type (0 ECPU, 1 MCPU, 2 PCPU).
fn cluster_tables(bytes: &[u8]) -> [Option<String>; 3] {
    let mut keys: [Option<String>; 3] = Default::default();
    for entry in bytes.chunks_exact(8) {
        if let Some(key) = keys.get_mut(usize::from(entry[1])) {
            key.get_or_insert_with(|| format!("voltage-states{}-sram", entry[0]));
        }
    }
    keys
}

/// Decodes `[u32 frequency][u32 voltage]` pairs, skipping the OFF state.
/// M1–M3 store Hz; M4 and later store kHz.
fn parse_voltage_states(bytes: &[u8]) -> Vec<u32> {
    let raw: Vec<u64> = bytes
        .chunks_exact(8)
        .map(|pair| u32::from_le_bytes(pair[..4].try_into().expect("4 bytes")) as u64)
        .filter(|&f| f > 0)
        .collect();
    let divisor = if raw.iter().any(|&f| f > 100_000_000) {
        1_000_000
    } else {
        1_000
    };
    raw.into_iter().map(|f| (f / divisor) as u32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::CoreKind;

    fn named(states: &[(&str, i64)]) -> Vec<(String, i64)> {
        states
            .iter()
            .map(|&(name, residency)| (name.to_owned(), residency))
            .collect()
    }

    /// Usage and clock of a GPU with these channels.
    fn gpu(channels: &[&[(&str, i64)]], freqs: &[u32]) -> (Option<f32>, Option<u32>) {
        let mut gpu = Dvfs::default();
        for states in channels {
            gpu.add(&named(states), freqs);
        }
        (gpu.usage(), gpu.mhz(freqs))
    }

    #[test]
    fn decodes_hz_and_khz_tables() {
        let hz: Vec<u8> = [0u32, 0, 600_000_000, 1, 2_064_000_000, 1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(parse_voltage_states(&hz), vec![600, 2064]);
        let khz: Vec<u8> = [1_020_000u32, 1, 4_512_000, 1]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(parse_voltage_states(&khz), vec![1020, 4512]);
    }

    #[test]
    fn weights_gpu_frequency_by_active_residency() {
        let states = [("OFF", 50), ("P1", 25), ("P2", 25)];
        assert_eq!(gpu(&[&states], &[400, 800]), (Some(50.0), Some(600)));
    }

    #[test]
    fn gpu_states_past_the_table_do_not_drag_its_clock_down() {
        // P3 has no frequency in the table, so only P1 and P2 weigh in: 600 MHz,
        // where dividing by all three would read 300.
        let states = [("OFF", 0), ("P1", 25), ("P2", 25), ("P3", 50)];
        assert_eq!(gpu(&[&states], &[400, 800]), (Some(100.0), Some(600)));
    }

    #[test]
    fn a_missing_frequency_table_is_no_clock_rather_than_0_mhz() {
        let states = [("OFF", 50), ("P1", 50)];
        assert_eq!(gpu(&[&states], &[]), (Some(50.0), None));
    }

    #[test]
    fn combines_the_gpu_channels_of_every_die() {
        let busy = [("OFF", 0), ("P1", 0), ("P2", 100)];
        let idle = [("OFF", 100), ("P1", 0), ("P2", 0)];
        assert_eq!(gpu(&[&busy, &idle], &[400, 800]), (Some(50.0), Some(800)));
        assert_eq!(gpu(&[&idle, &busy], &[400, 800]), (Some(50.0), Some(800)));
    }

    #[test]
    fn residencies_too_large_to_sum_do_not_overflow() {
        let maxed = [("OFF", i64::MAX), ("P1", i64::MAX)];
        assert_eq!(gpu(&[&maxed, &maxed], &[400]), (Some(50.0), Some(400)));
    }

    #[test]
    fn idle_states_after_the_active_ones_are_not_clocked() {
        let freqs = [600, 1200];
        let mut core = Dvfs::default();
        // Slicing from the first active state would pair DOWN with 1200 MHz and read 900.
        core.add(
            &named(&[("IDLE", 50), ("V0P5", 25), ("DOWN", 25), ("V1P4", 0)]),
            &freqs,
        );
        assert_eq!(core.mhz(&freqs), Some(600));
    }

    #[test]
    fn a_cluster_without_channels_has_no_clock_and_an_idle_one_its_lowest() {
        let freqs = [600, 2064];
        assert_eq!(Dvfs::default().mhz(&freqs), None);
        let mut idle = Dvfs::default();
        idle.add(&named(&[("IDLE", 100), ("V0P5", 0), ("V1P4", 0)]), &freqs);
        assert_eq!(idle.mhz(&freqs), Some(600));
    }

    #[test]
    fn sorts_core_channels_into_clusters_slowest_first() {
        assert_eq!(cpu_cluster("ECPU0"), Some(0));
        assert_eq!(cpu_cluster("EACC_ECPU0"), Some(0));
        assert_eq!(cpu_cluster("PACC0_MCPU2"), Some(1));
        assert_eq!(cpu_cluster("PCPU1"), Some(2));
        assert_eq!(cpu_cluster("DIE_1_PCPU1_CPU0"), Some(2));
        assert_eq!(cpu_cluster("GPUPH"), None);
    }

    #[test]
    fn reads_each_cluster_table_from_acc_clusters() {
        // An M5 Max: tables 22, 23 and 5 for its ECPU, MCPU and PCPU clusters.
        let bytes = [
            22, 0, 0, 0, 0, 0, 0, 0, 23, 1, 0, 0, 0, 0, 0, 0, 5, 2, 0, 0, 0, 0, 0, 0,
        ];
        let keys = [
            "voltage-states22-sram",
            "voltage-states23-sram",
            "voltage-states5-sram",
        ];
        assert_eq!(cluster_tables(&bytes), keys.map(|key| Some(key.to_owned())));
        assert_eq!(
            cluster_tables(&[9, 7, 0, 0, 0, 0, 0, 0]),
            [None, None, None]
        );
    }

    #[test]
    fn pairs_cluster_clocks_with_core_tiers_in_order() {
        // An M5 Pro: MCPU Performance cores, then PCPU Super cores.
        let tiers = [
            CoreTier {
                kind: CoreKind::Performance,
                cores: 12,
            },
            CoreTier {
                kind: CoreKind::Super,
                cores: 6,
            },
        ];
        let clocks = |cpu_mhz| {
            let reading = Reading {
                cpu_mhz,
                ..Reading::default()
            };
            reading.tier_clocks(&tiers)
        };
        let clock = |kind, mhz| TierClock { kind, mhz };
        assert_eq!(
            clocks(vec![Some(3000), Some(4200)]),
            [
                clock(CoreKind::Performance, 3000),
                clock(CoreKind::Super, 4200)
            ]
        );
        assert_eq!(clocks(vec![Some(3000), None]), [], "a tier without a table");
        assert_eq!(clocks(vec![Some(4200)]), [], "a tier without channels");
    }

    #[test]
    fn a_block_without_a_channel_reports_nothing() {
        let mut counts = EnergyCounts::default();
        counts.add("ECPU0 CPU Energy", Some(0.25));
        counts.add("PCPU CPU Energy", Some(1.0));
        counts.add("GPU Energy", Some(2.0));
        counts.add("DRAM", Some(0.5));
        counts.add("DISP", Some(0.5));
        let energy = counts.watts(1.0);
        assert_eq!(energy.cpu, Some(1.25));
        assert_eq!(energy.gpu, Some(2.0));
        assert_eq!(energy.dram, Some(0.5));
        assert_eq!(energy.ane, None, "no ANE channel, not 0 W");
    }

    #[test]
    fn a_counter_that_went_backwards_is_no_reading() {
        let mut counts = EnergyCounts::default();
        counts.add("ECPU CPU Energy", Some(1.0));
        // A counter reset across sleep, then IOReport's INT64_MIN sentinel.
        counts.add("PCPU CPU Energy", Some(-3.2));
        counts.add("GPU Energy", Some(i64::MIN as f64 / 1e6));
        let energy = counts.watts(1.0);
        assert_eq!((energy.cpu, energy.gpu), (None, None));
    }

    #[test]
    fn dead_counters_are_no_reading_but_an_idle_neural_engine_draws_0_w() {
        let energy = |cpu, ane, dram| {
            let mut counts = EnergyCounts::default();
            counts.add("CPU Energy", Some(cpu));
            counts.add("ANE", Some(ane));
            counts.add("DRAM", Some(dram));
            counts.add("GPU Energy", Some(1.5));
            counts.watts(1.0)
        };
        let idle = energy(2.0, 0.0, 0.5);
        assert_eq!(
            (idle.cpu, idle.ane, idle.dram),
            (Some(2.0), Some(0.0), Some(0.5))
        );
        // macOS 27 betas freeze the CPU, ANE and DRAM counters.
        let dead = energy(0.0, 0.0, 0.0);
        assert_eq!((dead.cpu, dead.ane, dead.dram), (None, None, None));
        assert_eq!(dead.gpu, Some(1.5));
    }

    #[test]
    fn an_unknown_energy_unit_is_no_reading_rather_than_a_guess() {
        assert_eq!(unit_divisor("mJ"), Some(1e3));
        assert_eq!(unit_divisor("pJ"), Some(1e12));
        assert_eq!(unit_divisor("24Mticks"), None);
        let mut counts = EnergyCounts::default();
        counts.add("GPU Energy", Some(1.0));
        counts.add("GPU Energy", None);
        assert_eq!(counts.watts(1.0).gpu, None);
    }

    #[test]
    fn counts_only_the_neural_engine_byte_channels() {
        assert!(is_ane_traffic("ANE0 RD", "B"));
        assert!(is_ane_traffic("ANE1 WR", "B"));
        assert!(!is_ane_traffic("ANE0 DCS RD", "B"), "the same bytes again");
        assert!(
            !is_ane_traffic("ANE0 RD+WR", "events"),
            "PMP's uninitialized twin"
        );
        assert!(!is_ane_traffic("GFX RD", "B"));
    }
}
