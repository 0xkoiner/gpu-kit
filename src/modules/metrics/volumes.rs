//! Mounted filesystems from /proc/self/mounts, sized with `statvfs`.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::snapshot::Volume;
use crate::{files, sys};

const MOUNTS: &str = "/proc/self/mounts";
const BY_LABEL: &str = "/dev/disk/by-label";

/// The filesystems a user browses: the root and the other mounts of block
/// devices. `None` when the mount table can't be read.
pub fn read() -> Option<Vec<Volume>> {
    let mounts = parse_mounts(&fs::read_to_string(MOUNTS).ok()?);
    let labels = labels(Path::new(BY_LABEL));
    let mut seen_devices = HashSet::new();
    let volumes = mounts
        .into_iter()
        .filter(is_listed)
        .filter_map(|mount| {
            let (total, available) = sizes(&mount.mount_point)?;
            // A device at two mount points (a bind mount) is listed once; checked
            // last, so a mount left out above can't hide the one that counts.
            let device =
                fs::canonicalize(&mount.source).unwrap_or_else(|_| PathBuf::from(&mount.source));
            if total == 0 || !seen_devices.insert(device.clone()) {
                return None;
            }
            Some(Volume {
                name: labels
                    .get(&device)
                    .cloned()
                    .unwrap_or_else(|| name_of(&mount.mount_point)),
                mount_point: mount.mount_point,
                total,
                available,
            })
        })
        .collect();
    Some(volumes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mount {
    source: String,
    mount_point: String,
}

/// The mounts of /proc/self/mounts, whose lines go `source mount_point type
/// options dump pass`, with spaces and the like in the first two written as
/// `\ooo` octal escapes.
fn parse_mounts(mounts: &str) -> Vec<Mount> {
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(Mount {
                source: unescape_octal(fields.next()?),
                mount_point: unescape_octal(fields.next()?),
            })
        })
        .collect()
}

fn unescape_octal(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escape = bytes.get(at..at + 4).filter(|window| {
            window[0] == b'\\'
                && window[1..]
                    .iter()
                    .all(|digit| (b'0'..=b'7').contains(digit))
        });
        match escape {
            Some(window) => {
                let code = window[1..]
                    .iter()
                    .fold(0u32, |code, &digit| code * 8 + u32::from(digit - b'0'));
                out.push(u8::try_from(code).unwrap_or(b'?'));
                at += 4;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a mount is one a user browses: a filesystem on a block device
/// (`/dev/...`), the root among them, that isn't a loop device (every snap on
/// Ubuntu mounts a squashfs from one) and doesn't sit under /boot, where the
/// EFI and boot partitions are the firmware's. Pseudo filesystems (proc,
/// sysfs, cgroup, tmpfs, overlay) have no device. Network mounts are left out
/// as well: `statvfs` on one whose server has gone away blocks until it comes
/// back, and Linux has no `MNT_NOWAIT` to ask for cached sizes instead.
fn is_listed(mount: &Mount) -> bool {
    mount.source.starts_with("/dev/")
        && !mount.source.starts_with("/dev/loop")
        && !Path::new(&mount.mount_point).starts_with("/boot")
}

/// Total and available bytes of the filesystem at `mount_point`. `None` when
/// `statvfs` fails (a mount that just went away) or the sizes overflow, as
/// only a misreporting filesystem makes them.
fn sizes(mount_point: &str) -> Option<(u64, u64)> {
    let path = CString::new(mount_point).ok()?;
    let mut stat = sys::statvfs::default();
    // `path` is NUL-terminated and `stat` is the struct the call fills.
    if unsafe { sys::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let fragment = stat.f_frsize;
    Some((
        stat.f_blocks.checked_mul(fragment)?,
        stat.f_bavail.checked_mul(fragment)?,
    ))
}

/// Filesystem labels by the device they're on, from udev's links under
/// /dev/disk/by-label, whose names write awkward bytes as `\xHH`.
fn labels(by_label: &Path) -> HashMap<PathBuf, String> {
    files::entries(by_label)
        .into_iter()
        .filter_map(|link| {
            let device = fs::canonicalize(&link).ok()?;
            Some((device, unescape_udev(files::name(&link)?)))
        })
        .collect()
}

fn unescape_udev(name: &str) -> String {
    let mut out = Vec::with_capacity(name.len());
    let mut rest = name.as_bytes();
    while let Some(&byte) = rest.first() {
        let escaped = (byte == b'\\' && rest.len() >= 4 && rest[1] == b'x')
            .then(|| std::str::from_utf8(&rest[2..4]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                rest = &rest[4..];
            }
            None => {
                out.push(byte);
                rest = &rest[1..];
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A mount's name when its filesystem has no label: the folder it's mounted
/// at, or "/" for the root.
fn name_of(mount_point: &str) -> String {
    mount_point
        .rsplit('/')
        .next()
        .filter(|folder| !folder.is_empty())
        .unwrap_or(mount_point)
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNT_TABLE: &str = "proc /proc proc rw,relatime 0 0
/dev/vda / ext4 rw,relatime,discard 0 0
/dev/vda / ext4 rw,relatime,discard 0 0
/dev/nvme0n1p15 /boot/efi vfat rw 0 0
/dev/loop3 /snap/core22/1612 squashfs ro,nodev 0 0
/dev/sdb1 /media/me/My\\040Disk exfat rw 0 0
server:/export /mnt/nfs nfs4 rw 0 0
";

    #[test]
    fn mounts_parse_with_their_escapes_decoded() {
        let mounts = parse_mounts(MOUNT_TABLE);
        assert_eq!(mounts.len(), 7);
        assert_eq!(mounts[5].source, "/dev/sdb1");
        assert_eq!(mounts[5].mount_point, "/media/me/My Disk");
        assert_eq!(unescape_octal("a\\011b\\"), "a\tb\\");
        assert_eq!(unescape_octal("\\0409"), " 9");
    }

    #[test]
    fn only_block_device_mounts_outside_boot_are_listed() {
        let listed: Vec<String> = parse_mounts(MOUNT_TABLE)
            .into_iter()
            .filter(is_listed)
            .map(|mount| mount.mount_point)
            .collect();
        assert_eq!(listed, ["/", "/", "/media/me/My Disk"]);
    }

    #[test]
    fn udev_label_names_are_decoded() {
        assert_eq!(unescape_udev("My\\x20Disk"), "My Disk");
        assert_eq!(unescape_udev("plain"), "plain");
        assert_eq!(unescape_udev("odd\\x2"), "odd\\x2");
    }

    #[test]
    fn unlabelled_volumes_go_by_their_folder() {
        assert_eq!(name_of("/"), "/");
        assert_eq!(name_of("/media/me/My Disk"), "My Disk");
    }

    #[test]
    fn the_root_filesystem_has_a_size() {
        let (total, available) = sizes("/").expect("statvfs on /");
        assert!(total > 0 && available <= total, "{total} {available}");
        let volumes = read().expect("/proc/self/mounts");
        if Path::new("/dev/vda").exists() || Path::new("/dev/sda").exists() {
            assert!(volumes.iter().any(|v| v.mount_point == "/"), "{volumes:?}");
        }
    }
}
