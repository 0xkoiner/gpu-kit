//! Memory and swap from /proc/meminfo.

use crate::files;
use crate::snapshot::{Memory, Swap};

/// `None` when /proc/meminfo can't be read or has no `MemAvailable`
/// (kernels before 3.14).
pub fn read() -> Option<Memory> {
    parse(&files::read("/proc/meminfo")?)
}

/// Physical memory in bytes, the whole each process's share is of.
pub fn total_bytes() -> Option<u64> {
    kib(&files::read("/proc/meminfo")?, "MemTotal")
}

fn kib(meminfo: &str, key: &str) -> Option<u64> {
    files::kib_to_bytes(files::field(meminfo, key)?)
}

/// Used memory is what `MemAvailable` leaves of the total: the kernel's own
/// estimate of what a new workload could take without swapping, which counts
/// reclaimable cache as free, as `free` does.
fn parse(meminfo: &str) -> Option<Memory> {
    let total = kib(meminfo, "MemTotal")?;
    let available = kib(meminfo, "MemAvailable")?;
    let swap = kib(meminfo, "SwapTotal")
        .zip(kib(meminfo, "SwapFree"))
        .map(|(total, free)| Swap {
            used: total.saturating_sub(free),
            total,
        });
    Some(Memory {
        used: total.saturating_sub(available),
        total,
        swap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_is_what_available_leaves_of_the_total() {
        let meminfo = "MemTotal:       16479424 kB
MemFree:        15929928 kB
MemAvailable:   15938452 kB
SwapTotal:       2097148 kB
SwapFree:        2000000 kB
";
        let memory = parse(meminfo).expect("a complete meminfo");
        assert_eq!(memory.total, 16_479_424 * 1024);
        assert_eq!(memory.used, (16_479_424 - 15_938_452) * 1024);
        let swap = memory.swap.expect("swap figures");
        assert_eq!((swap.used, swap.total), (97_148 * 1024, 2_097_148 * 1024));
    }

    #[test]
    fn without_mem_available_there_is_no_reading() {
        assert!(parse("MemTotal: 100 kB\nMemFree: 50 kB\n").is_none());
    }

    #[test]
    fn this_machine_has_memory() {
        let memory = read().expect("/proc/meminfo");
        assert!(memory.total > 0 && memory.used <= memory.total);
        assert_eq!(total_bytes(), Some(memory.total));
    }
}
