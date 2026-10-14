//! Reading the small text files /proc and /sys serve.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A file's text, trimmed. `None` when it can't be read: a driver that isn't
/// loaded, a process that exited, or a value the kernel keeps from
/// unprivileged users.
pub fn read(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_owned())
}

/// A file holding one number.
pub fn read_number<T: FromStr>(path: impl AsRef<Path>) -> Option<T> {
    read(path)?.parse().ok()
}

/// The entries of a directory, sorted by name; empty when it can't be listed.
pub fn entries(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    entries
}

/// The file name of a path, when it's text.
pub fn name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

/// The first value of `key` in a file of `key: value` lines, as /proc/meminfo
/// and /proc/cpuinfo are laid out.
pub fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim())
    })
}

/// Bytes from a value /proc prints in kibibytes, such as `16384000 kB`.
pub fn kib_to_bytes(value: &str) -> Option<u64> {
    let kib: u64 = value.strip_suffix("kB")?.trim().parse().ok()?;
    kib.checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_are_found_by_key_with_the_value_trimmed() {
        let text = "MemTotal:       16479424 kB\nMemFree:  15929928 kB\ncpu MHz\t\t: 2700.000\n";
        assert_eq!(field(text, "MemFree"), Some("15929928 kB"));
        assert_eq!(field(text, "cpu MHz"), Some("2700.000"));
        assert_eq!(field(text, "Mem"), None);
    }

    #[test]
    fn kibibytes_become_bytes() {
        assert_eq!(kib_to_bytes("16479424 kB"), Some(16_874_930_176));
        assert_eq!(kib_to_bytes("16479424"), None);
        assert_eq!(kib_to_bytes(&format!("{} kB", u64::MAX)), None);
    }
}
