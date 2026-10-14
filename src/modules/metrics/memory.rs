use crate::modules::metrics::snapshot::Memory;
use crate::modules::metrics::sys;

/// `None` when the kernel's page counts can't be read, so a failed read is
/// not mistaken for idle memory.
pub fn read(total: u64) -> Option<Memory> {
    let swap = sys::sysctl_value::<sys::xsw_usage>("vm.swapusage").unwrap_or_default();
    memory_from(total, vm_statistics(), swap)
}

fn memory_from(
    total: u64,
    stats: Option<sys::vm_statistics64>,
    swap: sys::xsw_usage,
) -> Option<Memory> {
    let stats = stats?;
    // mactop (via gopsutil): everything except free and inactive pages is used.
    let available = (stats.free_count as u64 + stats.inactive_count as u64) * kernel_page_size();
    Some(Memory {
        used: total.saturating_sub(available),
        total,
        swap_used: swap.xsu_used,
        swap_total: swap.xsu_total,
    })
}

/// The kernel's page counts, in units of [`kernel_page_size`].
fn vm_statistics() -> Option<sys::vm_statistics64> {
    let mut stats = sys::vm_statistics64::default();
    let mut count =
        (size_of::<sys::vm_statistics64>() / size_of::<sys::integer_t>()) as sys::natural_t;
    let rc = unsafe {
        sys::host_statistics64(
            sys::host_port(),
            sys::HOST_VM_INFO64,
            (&mut stats as *mut sys::vm_statistics64).cast(),
            &mut count,
        )
    };
    (rc == sys::KERN_SUCCESS).then_some(stats)
}

/// The kernel's page size. Not `hw.pagesize`: that is the process's, 4 KiB
/// under Rosetta while the kernel counts 16 KiB pages.
fn kernel_page_size() -> u64 {
    unsafe { sys::vm_kernel_page_size as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_counts_at_the_kernel_page_size_add_up_to_physical_memory() {
        let memsize = sys::sysctl_value::<u64>("hw.memsize").expect("hw.memsize unreadable");
        let stats = vm_statistics().expect("host_statistics64 failed");
        let pages: u64 = [
            stats.free_count,
            stats.active_count,
            stats.inactive_count,
            stats.wire_count,
            stats.compressor_page_count,
        ]
        .into_iter()
        .map(u64::from)
        .sum();
        let ratio = (pages * kernel_page_size()) as f64 / memsize as f64;
        // The kernel keeps some memory off its page lists, so the counts fall a
        // little short. A wrong unit, like Rosetta's `hw.pagesize`, is off fourfold.
        assert!(
            (0.5..1.5).contains(&ratio),
            "page counts cover {ratio:.2} of hw.memsize"
        );
    }

    #[test]
    fn failed_vm_read_is_no_reading_rather_than_zero_used() {
        // `vm_statistics` returns `None` when the kernel call fails.
        assert!(memory_from(1 << 34, None, sys::xsw_usage::default()).is_none());
    }
}