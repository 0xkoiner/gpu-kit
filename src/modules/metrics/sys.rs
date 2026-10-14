//! Raw declarations for the glibc calls this crate needs on Linux: the few
//! things /proc and /sys don't serve as text. Layouts follow the LP64 ABI
//! that x86_64 and aarch64 Linux share.
#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_long, c_ulong, c_void};

const _: () = assert!(
    size_of::<usize>() == 8,
    "layouts follow 64-bit (LP64) glibc"
);

// <unistd.h>
pub const _SC_CLK_TCK: c_int = 2;
pub const _SC_PAGESIZE: c_int = 30;

// <errno.h>
pub const EPERM: c_int = 1;
pub const ESRCH: c_int = 3;
pub const EINVAL: c_int = 22;

// <signal.h>
pub const SIGKILL: c_int = 9;
pub const SIGTERM: c_int = 15;

// <dlfcn.h>
pub const RTLD_NOW: c_int = 2;

/// `struct statvfs` from <bits/statvfs.h> on 64-bit glibc; musl lays it out
/// the same way.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct statvfs {
    pub f_bsize: c_ulong,
    /// The unit of the block counts below.
    pub f_frsize: c_ulong,
    pub f_blocks: u64,
    pub f_bfree: u64,
    /// Blocks free to an unprivileged user, after the root reserve.
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_favail: u64,
    pub f_fsid: c_ulong,
    pub f_flag: c_ulong,
    pub f_namemax: c_ulong,
    f_spare: [c_int; 6],
}

const _: () = assert!(size_of::<statvfs>() == 112);

unsafe extern "C" {
    pub fn statvfs(path: *const c_char, buf: *mut statvfs) -> c_int;
    pub fn sysconf(name: c_int) -> c_long;
    /// Private: callers go through [`send_signal`].
    fn kill(pid: c_int, sig: c_int) -> c_int;
    pub fn __errno_location() -> *mut c_int;
}

// libdl is part of libc since glibc 2.34; the stub library keeps `-ldl` working.
#[link(name = "dl")]
unsafe extern "C" {
    pub fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    pub fn dlclose(handle: *mut c_void) -> c_int;
}

pub fn errno() -> c_int {
    unsafe { *__errno_location() }
}

/// A `sysconf` value, or `None` when the system doesn't define it.
pub fn sysconf_value(name: c_int) -> Option<u64> {
    u64::try_from(unsafe { sysconf(name) }).ok()
}

/// Sends `sig` to the single process `pid`, or returns the errno. A pid of 0
/// would signal our own process group, -1 every process we may signal and
/// any other negative pid a process group, so none is accepted: `EINVAL`.
/// Guarding against pid reuse is the caller's job.
pub fn send_signal(pid: c_int, sig: c_int) -> Result<(), c_int> {
    if pid <= 0 {
        return Err(EINVAL);
    }
    if unsafe { kill(pid, sig) } == 0 {
        Ok(())
    } else {
        Err(errno())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signalling_our_group_or_every_process_is_refused() {
        // Signal 0 only checks the target, so even a broken guard sends nothing.
        assert_eq!(send_signal(0, 0), Err(EINVAL));
        assert_eq!(send_signal(-1, 0), Err(EINVAL));
    }

    #[test]
    fn signal_reaches_a_single_process() {
        assert_eq!(send_signal(std::process::id() as c_int, 0), Ok(()));
    }

    #[test]
    fn the_clock_tick_rate_is_defined() {
        assert!(sysconf_value(_SC_CLK_TCK).is_some_and(|ticks| ticks > 0));
    }
}
