//! Block device byte counters from /proc/diskstats; rates come from deltas.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::files;
use crate::rate::{ByteCounts, ByteRates};
use crate::snapshot::Throughput;

const BLOCK: &str = "/sys/block";

/// /proc/diskstats counts 512-byte sectors whatever the device's own sector size.
const SECTOR: u64 = 512;

/// Read/write byte counters of each physical disk; rates come from deltas.
#[derive(Default)]
pub struct DiskCounters {
    rates: ByteRates<String>,
}

impl DiskCounters {
    /// `None` on the first call, which only sets the baseline, and when no
    /// physical disk's counters can be read.
    pub fn sample(&mut self) -> Option<Throughput> {
        self.rates.sample(Instant::now(), read_disks())
    }
}

/// Counters of each physical disk by name. `None` when /proc/diskstats can't
/// be read or lists no physical disk: every machine boots from one, though a
/// container may be shown none.
fn read_disks() -> Option<HashMap<String, ByteCounts>> {
    let disks: HashMap<String, ByteCounts> = parse_counts(&files::read("/proc/diskstats")?)
        .into_iter()
        .filter(|(name, _)| is_physical_disk(Path::new(BLOCK), name))
        .collect();
    (!disks.is_empty()).then_some(disks)
}

/// `(name, bytes)` of every device in /proc/diskstats, whose fields are
/// major, minor, name, then reads completed, reads merged, sectors read, ms
/// reading, writes completed, writes merged, sectors written, and more.
fn parse_counts(diskstats: &str) -> Vec<(String, ByteCounts)> {
    diskstats
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let bytes = |ix: usize| fields.get(ix)?.parse::<u64>().ok()?.checked_mul(SECTOR);
            let counts = ByteCounts {
                read: bytes(5)?,
                written: bytes(9)?,
            };
            Some((fields.get(2)?.to_string(), counts))
        })
        .collect()
}

/// Whether `name` is a whole physical disk: it has its own entry under
/// /sys/block (a partition's sits under its disk's) that isn't a virtual
/// device. Loop devices (every snap on Ubuntu mounts one), device-mapper
/// (LVM, LUKS), md RAID and zram are virtual: their I/O lands on the disks
/// beneath them, so counting both would double it, and a RAM disk's I/O is no
/// disk traffic. An NVMe multipath system also lists each path (`nvme0c0n1`)
/// beside the disk it reaches (`nvme0n1`); the paths are skipped.
fn is_physical_disk(block: &Path, name: &str) -> bool {
    if name.contains('/') || is_nvme_path(name) {
        return false;
    }
    let Some(virtual_devices) = block.parent().map(|sys| sys.join("devices/virtual")) else {
        return false;
    };
    std::fs::canonicalize(block.join(name)).is_ok_and(|device| !device.starts_with(virtual_devices))
}

/// `nvme<ctrl>c<path>n<ns>` names one path to a namespace, not the namespace.
fn is_nvme_path(name: &str) -> bool {
    name.strip_prefix("nvme")
        .map(|rest| rest.trim_start_matches(|c: char| c.is_ascii_digit()))
        .is_some_and(|rest| rest.starts_with('c'))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    const DISKSTATS: &str = "   7       0 loop0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
 254       0 vda 21524 4420 2625050 7615 2348 1582 675904 2185 0 6892 10233 658 0 8600 416 29 15
 254       1 vda1 21000 4420 2600000 7600 2300 1582 675000 2185 0 6892 10233 0 0 0 0 0 0
";

    #[test]
    fn sectors_become_bytes_per_device() {
        let counts = parse_counts(DISKSTATS);
        assert_eq!(counts.len(), 3);
        assert_eq!(counts[1].0, "vda");
        assert_eq!(
            (counts[1].1.read, counts[1].1.written),
            (2_625_050 * 512, 675_904 * 512)
        );
    }

    /// A fake /sys with a physical disk and a loop device, as symlinks into
    /// `devices`, the way sysfs lays /sys/block out.
    fn fake_sys() -> PathBuf {
        let sys = std::env::temp_dir().join(format!("top-metrics-disk-{}", std::process::id()));
        let _ = fs::remove_dir_all(&sys);
        for dir in [
            "block",
            "devices/virtual/block/loop0",
            "devices/pci0000:00/virtio1/block/vda",
        ] {
            fs::create_dir_all(sys.join(dir)).expect("fixture dir");
        }
        std::os::unix::fs::symlink("../devices/virtual/block/loop0", sys.join("block/loop0"))
            .expect("loop0 link");
        std::os::unix::fs::symlink(
            "../devices/pci0000:00/virtio1/block/vda",
            sys.join("block/vda"),
        )
        .expect("vda link");
        sys
    }

    #[test]
    fn partitions_and_virtual_devices_are_not_disks() {
        let sys = fake_sys();
        let block = sys.join("block");
        assert!(is_physical_disk(&block, "vda"));
        assert!(!is_physical_disk(&block, "vda1"), "no entry of its own");
        assert!(!is_physical_disk(&block, "loop0"), "virtual");
        assert!(!is_physical_disk(&block, "../devices"), "not a name");
        let _ = fs::remove_dir_all(sys);
    }

    #[test]
    fn nvme_paths_are_told_from_namespaces() {
        assert!(is_nvme_path("nvme0c0n1"));
        assert!(!is_nvme_path("nvme0n1"));
        assert!(!is_nvme_path("nvme10n2"));
        assert!(!is_nvme_path("sda"));
    }

    #[test]
    fn this_machine_boots_from_a_physical_disk() {
        if Path::new(BLOCK).exists() {
            assert!(read_disks().is_some_and(|disks| !disks.is_empty()));
        }
    }
}
