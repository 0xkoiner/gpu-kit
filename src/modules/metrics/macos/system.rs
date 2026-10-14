use std::time::{SystemTime, UNIX_EPOCH};

use super::cf;
use super::cpu;
use super::sys;
use crate::snapshot::{Battery, ChipInfo, ThermalPressure};

/// The chip's name, core tiers, GPU core count and memory size. The name falls
/// back to "Apple Silicon" and the counts to 0 when macOS doesn't report them.
pub fn chip_info() -> ChipInfo {
    ChipInfo {
        name: sys::sysctl_string("machdep.cpu.brand_string")
            .unwrap_or_else(|| "Apple Silicon".into()),
        tiers: cpu::core_tiers(),
        gpu_cores: gpu_core_count().unwrap_or(0),
        memory_bytes: sys::sysctl_value::<u64>("hw.memsize").unwrap_or(0),
    }
}

/// From the first `AGXAccelerator` registry entry that states a count, as
/// `ioreg -rc AGXAccelerator` lists them.
fn gpu_core_count() -> Option<u32> {
    let mut cores = None;
    cf::for_each_service("AGXAccelerator", |accelerator| {
        if cores.is_none() {
            cores = int_property(accelerator, "gpu-core-count")
                .and_then(|count| u32::try_from(count).ok());
        }
    });
    cores
}

/// Seconds since boot, sleep included, as `uptime` counts them. 0 when the
/// boot time can't be read.
pub fn uptime_secs() -> u64 {
    let boot = sys::sysctl_value::<sys::timeval>("kern.boottime").map(|t| t.tv_sec);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    uptime_since(boot, now)
}

/// Zero when the boot time is unknown, rather than the time since 1970.
fn uptime_since(boot: Option<i64>, now: u64) -> u64 {
    boot.filter(|&secs| secs > 0)
        .map_or(0, |secs| now.saturating_sub(secs as u64))
}

/// The kernel's thermal pressure level, read through libnotify. `None` when
/// the notify state can't be read or is a level macOS doesn't define.
pub fn thermal_pressure() -> Option<ThermalPressure> {
    let name = c"com.apple.system.thermalpressurelevel";
    let mut token = 0;
    let mut level = 0u64;
    // The out-pointers are locals of the types <notify.h> declares, and a
    // registered token is cancelled whether or not its state could be read.
    let status = unsafe {
        if sys::notify_register_check(name.as_ptr(), &mut token) != 0 {
            return None;
        }
        let status = sys::notify_get_state(token, &mut level);
        sys::notify_cancel(token);
        status
    };
    if status != 0 {
        return None;
    }
    ThermalPressure::try_from(level).ok()
}

/// Charge and power source of the internal battery, from its
/// `AppleSmartBattery` registry entry. `None` on a Mac without one, or when
/// its capacity can't be read.
pub fn battery() -> Option<Battery> {
    let mut battery = None;
    cf::for_each_service("AppleSmartBattery", |entry| {
        let capacity = (
            int_property(entry, "CurrentCapacity"),
            int_property(entry, "MaxCapacity"),
        );
        if let (Some(current), Some(max)) = capacity
            && let Some(percent) = percent(current, max)
        {
            battery = Some(Battery {
                percent,
                on_ac: cf::property(entry, "ExternalConnected")
                    .is_some_and(|value| cf::is_true(&value)),
            });
        }
    });
    battery
}

/// `current` as a share of `max`, clamped to 0–100. `None` when `max` isn't
/// positive or the capacities are too large to scale.
fn percent(current: i64, max: i64) -> Option<u32> {
    if max <= 0 {
        return None;
    }
    // Clamped to 0–100, so the cast can't wrap.
    Some((current.checked_mul(100)? / max).clamp(0, 100) as u32)
}

/// An integer property of a registry entry.
fn int_property(entry: cf::io_object_t, key: &str) -> Option<i64> {
    let value = cf::property(entry, key)?;
    cf::number_i64(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_boot_time_is_not_counted_from_1970() {
        assert_eq!(uptime_since(None, 1_760_000_000), 0);
        assert_eq!(uptime_since(Some(1_760_000_000 - 90), 1_760_000_000), 90);
    }

    #[test]
    fn battery_percent_is_clamped_and_never_divides_by_zero() {
        assert_eq!(percent(42, 100), Some(42));
        assert_eq!(percent(3_000, 6_000), Some(50));
        assert_eq!(percent(120, 100), Some(100));
        assert_eq!(percent(-5, 100), Some(0));
        assert_eq!(percent(50, 0), None);
    }

    #[test]
    fn battery_capacities_too_large_to_scale_are_rejected() {
        assert_eq!(percent(i64::MAX, i64::MAX), None);
    }
}
