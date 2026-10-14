use super::sys;
use crate::snapshot::{CoreKind, CoreTier};

/// Per-core tick counters; usage is the busy share of the delta between reads.
#[derive(Default)]
pub struct CpuTicks {
    previous: Vec<[u32; 4]>,
}

impl CpuTicks {
    /// Returns busy shares, 0–100, in logical CPU order. Empty when the read
    /// fails, and when there is no baseline for these cores yet (the first
    /// call, or after the core count changes), which this read then becomes.
    pub fn sample(&mut self) -> Vec<f32> {
        self.sample_from(read_ticks())
    }

    fn sample_from(&mut self, current: Vec<[u32; 4]>) -> Vec<f32> {
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

/// Busy share, 0–100, between two reads of one core's tick counters.
/// The kernel counters are 32-bit, so deltas wrap rather than saturate.
fn busy_share(before: [u32; 4], now: [u32; 4]) -> f32 {
    let delta = |state: usize| now[state].wrapping_sub(before[state]) as f64;
    let busy =
        delta(sys::CPU_STATE_USER) + delta(sys::CPU_STATE_SYSTEM) + delta(sys::CPU_STATE_NICE);
    let total = busy + delta(sys::CPU_STATE_IDLE);
    if total > 0.0 {
        (busy / total * 100.0) as f32
    } else {
        0.0
    }
}

/// The chip's core tiers in logical CPU order. Apple Silicon numbers its
/// slowest tier first (the M5 Pro's 12 Performance cores are CPUs 0–11, its
/// 6 Super cores 12–17), while `hw.perflevel0` is the fastest, so the levels
/// are read in reverse. Empty when macOS reports no performance levels.
pub fn core_tiers() -> Vec<CoreTier> {
    let levels = sys::sysctl_value::<u32>("hw.nperflevels").unwrap_or(0);
    // All or nothing: a missing level would shift every tier after it.
    (0..levels)
        .rev()
        .map(|level| {
            let cores = sys::sysctl_value::<u32>(&format!("hw.perflevel{level}.logicalcpu"))?;
            let name = sys::sysctl_string(&format!("hw.perflevel{level}.name"));
            let kind = match name.as_deref() {
                Some("Efficiency") => CoreKind::Efficiency,
                Some("Super") => CoreKind::Super,
                _ => CoreKind::Performance,
            };
            Some(CoreTier { kind, cores })
        })
        .collect::<Option<_>>()
        .unwrap_or_default()
}

fn read_ticks() -> Vec<[u32; 4]> {
    let mut cpu_count: sys::natural_t = 0;
    let mut info: *mut sys::integer_t = std::ptr::null_mut();
    let mut info_count: sys::natural_t = 0;
    let rc = unsafe {
        sys::host_processor_info(
            sys::host_port(),
            sys::PROCESSOR_CPU_LOAD_INFO,
            &mut cpu_count,
            &mut info,
            &mut info_count,
        )
    };
    if rc != sys::KERN_SUCCESS || info.is_null() {
        return Vec::new();
    }
    let ticks = unsafe { std::slice::from_raw_parts(info as *const u32, info_count as usize) };
    let result = ticks
        .chunks_exact(4)
        .take(cpu_count as usize)
        .map(|t| [t[0], t[1], t[2], t[3]])
        .collect();
    unsafe {
        sys::vm_deallocate(
            sys::mach_task_self_,
            info as usize,
            info_count as usize * size_of::<sys::integer_t>(),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_counter_wraparound_is_not_read_as_full_load() {
        let before = [100, 50, u32::MAX - 9, 0];
        let after = [110, 50, 90, 0];
        // 10 busy ticks against 100 idle ticks across the wrap.
        assert!((busy_share(before, after) - 9.09).abs() < 0.01);
    }

    #[test]
    fn first_reading_only_sets_the_baseline() {
        let mut ticks = CpuTicks::default();
        // Since-boot counters are not an interval, so there is nothing to report yet.
        assert_eq!(
            ticks.sample_from(vec![[100, 50, 800, 0]]),
            Vec::<f32>::new()
        );
    }

    #[test]
    fn failed_read_keeps_the_baseline() {
        let mut ticks = CpuTicks::default();
        ticks.sample_from(vec![[100, 50, 800, 0]]);
        // `read_ticks` returns no cores when the kernel call fails.
        assert_eq!(ticks.sample_from(Vec::new()), Vec::<f32>::new());
        // 10 busy ticks against 90 idle ticks since the last good read.
        assert_eq!(ticks.sample_from(vec![[110, 50, 890, 0]]), vec![10.0]);
    }

    #[test]
    fn sampling_does_not_leak_host_port_references() {
        let host = sys::host_port();
        let refs = || {
            let mut refs = 0;
            let rc = unsafe {
                sys::mach_port_get_refs(
                    sys::mach_task_self_,
                    host,
                    sys::MACH_PORT_RIGHT_SEND,
                    &mut refs,
                )
            };
            // A failed call leaves `refs` at 0, which would pass vacuously.
            assert_eq!(rc, sys::KERN_SUCCESS, "mach_port_get_refs failed");
            refs
        };
        let mut ticks = CpuTicks::default();
        let before = refs();
        for _ in 0..50 {
            ticks.sample();
            crate::macos::memory::read(1 << 34);
        }
        // Tests running in parallel can hold a reference for a moment; a leak
        // would add one per call, a hundred here.
        assert!(refs() < before + 50, "{} → {}", before, refs());
    }
}
