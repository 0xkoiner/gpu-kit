use std::collections::HashSet;

use super::cf;
use super::sys;
use crate::snapshot::Volume;

/// The startup volume and the volumes Finder shows under `/Volumes`. `None`
/// when the mount table can't be read.
pub fn read() -> Option<Vec<Volume>> {
    let table = sys::MountTable::read()?;
    let mut seen_sources = HashSet::new();
    let volumes = table
        .entries()
        .iter()
        .filter_map(|mount| {
            let mount_point = sys::c_array_to_string(&mount.f_mntonname);
            let browsable = mount.f_flags & sys::MNT_DONTBROWSE == 0;
            let name = listed_name(&mount_point, browsable, startup_volume_name)?;
            let total = bytes(mount.f_blocks, mount.f_bsize)?;
            let available = bytes(mount.f_bavail, mount.f_bsize)?;
            // A device or share mounted at two points is listed once; checked
            // last, so a mount left out above can't hide the one that counts.
            if total == 0 || !seen_sources.insert(sys::c_array_to_string(&mount.f_mntfromname)) {
                return None;
            }
            Some(Volume {
                name,
                mount_point,
                total,
                available,
            })
        })
        .collect();
    Some(volumes)
}

/// The name a mount is listed under: `startup()` for `/`, or its folder for a
/// browsable volume mounted directly under `/Volumes`. `None` for any other
/// mount, and for those macOS marks `nobrowse` for Finder to hide (Preboot, VM
/// and the other system volumes). Going by the flag rather than the name means
/// a user's own disk called "VM" still shows.
fn listed_name(
    mount_point: &str,
    browsable: bool,
    startup: impl FnOnce() -> String,
) -> Option<String> {
    if mount_point == "/" {
        return Some(startup());
    }
    let folder = mount_point.strip_prefix("/Volumes/")?;
    (browsable && !folder.is_empty() && !folder.contains('/')).then(|| folder.to_owned())
}

/// The startup volume's name, or "Macintosh HD", the name macOS gives it, when
/// that can't be read.
fn startup_volume_name() -> String {
    cf::volume_name("/").unwrap_or_else(|| "Macintosh HD".to_owned())
}

/// `blocks` of `block_size` bytes. `None` when that overflows, as only a
/// misreporting filesystem can make it: a FUSE mount such as sshfs passes on
/// whatever sizes its server sends.
fn bytes(blocks: u64, block_size: u32) -> Option<u64> {
    blocks.checked_mul(u64::from(block_size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(mount_point: &str, browsable: bool) -> Option<String> {
        listed_name(mount_point, browsable, || "Startup".to_owned())
    }

    #[test]
    fn lists_the_startup_volume() {
        let volumes = read().expect("mount table");
        assert!(volumes.iter().any(|v| v.mount_point == "/" && v.total > 0));
    }

    #[test]
    fn the_startup_volume_goes_by_its_own_name() {
        assert_eq!(named("/", true).as_deref(), Some("Startup"));
    }

    #[test]
    fn browsable_volumes_go_by_their_folder() {
        assert_eq!(named("/Volumes/Backup", true).as_deref(), Some("Backup"));
        // Named like a system volume, but only the `nobrowse` flag hides one.
        assert_eq!(named("/Volumes/VM", true).as_deref(), Some("VM"));
    }

    #[test]
    fn hidden_nested_and_other_mounts_are_left_out() {
        assert_eq!(named("/Volumes/Recovery", false), None);
        assert_eq!(named("/Volumes/.timemachine/snapshot", true), None);
        assert_eq!(named("/Volumes/", true), None);
        assert_eq!(named("/System/Volumes/Data", true), None);
        assert_eq!(named("/dev", true), None);
    }

    #[test]
    fn sizes_too_large_to_count_in_bytes_are_rejected() {
        assert_eq!(bytes(1_000, 4096), Some(4_096_000));
        assert_eq!(bytes(u64::MAX / 2, 4096), None);
    }
}
