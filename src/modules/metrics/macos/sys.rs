//! Raw declarations for the macOS system libraries this crate reads.
//!
//! Layouts follow the arm64 macOS SDK headers named next to each item; where
//! x86_64 (Rosetta) differs, the item says so.
#![allow(non_camel_case_types, non_upper_case_globals, dead_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ptr;

pub type kern_return_t = c_int;
pub type mach_port_t = c_uint;
pub type natural_t = c_uint;
pub type integer_t = c_int;

// <mach/*.h>
pub const KERN_SUCCESS: kern_return_t = 0;
pub const PROCESSOR_CPU_LOAD_INFO: c_int = 2;
pub const CPU_STATE_USER: usize = 0;
pub const CPU_STATE_SYSTEM: usize = 1;
pub const CPU_STATE_IDLE: usize = 2;
pub const CPU_STATE_NICE: usize = 3;
pub const HOST_VM_INFO64: c_int = 4;

/// `struct vm_statistics64` from <mach/vm_statistics.h>, up to its rev1 fields.
/// Newer SDKs append more, so pass [`Self::COUNT`] to `host_statistics64`,
/// never the SDK's `HOST_VM_INFO64_COUNT`, or the kernel writes past the end.
#[repr(C)]
#[derive(Default)]
pub struct vm_statistics64 {
    pub free_count: natural_t,
    pub active_count: natural_t,
    pub inactive_count: natural_t,
    pub wire_count: natural_t,
    pub zero_fill_count: u64,
    pub reactivations: u64,
    pub pageins: u64,
    pub pageouts: u64,
    pub faults: u64,
    pub cow_faults: u64,
    pub lookups: u64,
    pub hits: u64,
    pub purges: u64,
    pub purgeable_count: natural_t,
    pub speculative_count: natural_t,
    pub decompressions: u64,
    pub compressions: u64,
    pub swapins: u64,
    pub swapouts: u64,
    pub compressor_page_count: natural_t,
    pub throttled_count: natural_t,
    pub external_page_count: natural_t,
    pub internal_page_count: natural_t,
    pub total_uncompressed_pages_in_compressor: u64,
}

impl vm_statistics64 {
    /// The size in `integer_t`s that `host_statistics64` takes as its count.
    pub const COUNT: natural_t = (size_of::<Self>() / size_of::<integer_t>()) as natural_t;
}

pub const HOST_VM_INFO64_REV1_COUNT: natural_t = 38;
const _: () = assert!(vm_statistics64::COUNT == HOST_VM_INFO64_REV1_COUNT);

/// `struct mach_timebase_info` from <mach/mach_time.h>.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct mach_timebase_info {
    pub numer: u32,
    pub denom: u32,
}

unsafe extern "C" {
    pub static mach_task_self_: mach_port_t;
    /// `vm_size_t` from <mach/vm_page_size.h>: the unit of the kernel's page counts.
    pub static vm_kernel_page_size: usize;
    pub fn mach_host_self() -> mach_port_t;
    pub fn host_processor_info(
        host: mach_port_t,
        flavor: c_int,
        out_processor_count: *mut natural_t,
        out_processor_info: *mut *mut integer_t,
        out_processor_info_count: *mut natural_t,
    ) -> kern_return_t;
    pub fn host_statistics64(
        host: mach_port_t,
        flavor: c_int,
        info: *mut integer_t,
        count: *mut natural_t,
    ) -> kern_return_t;
    pub fn vm_deallocate(task: mach_port_t, address: usize, size: usize) -> kern_return_t;
    pub fn mach_timebase_info(info: *mut mach_timebase_info) -> kern_return_t;
    pub fn mach_absolute_time() -> u64;
}

pub const MACH_PORT_RIGHT_SEND: u32 = 0;

unsafe extern "C" {
    pub fn mach_port_get_refs(
        task: mach_port_t,
        name: mach_port_t,
        right: u32,
        refs: *mut u32,
    ) -> kern_return_t;
}

/// The host port. Each `mach_host_self()` call adds a send-right reference,
/// so take one for the life of the process instead of one per sample.
pub fn host_port() -> mach_port_t {
    static HOST: std::sync::OnceLock<mach_port_t> = std::sync::OnceLock::new();
    *HOST.get_or_init(|| unsafe { mach_host_self() })
}

// <sys/sysctl.h>
pub const CTL_KERN: c_int = 1;
pub const CTL_NET: c_int = 4;
pub const KERN_PROC: c_int = 14;
pub const KERN_PROC_ALL: c_int = 0;
pub const KERN_PROC_PID: c_int = 1;
pub const PF_LINK: c_int = 18;
pub const ENOMEM: c_int = 12;

unsafe extern "C" {
    pub fn sysctl(
        name: *mut c_int,
        namelen: c_uint,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
    pub fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
}

/// A type the kernel may fill with raw bytes.
///
/// # Safety
/// Every bit pattern must be a valid `Self`: integers, or `#[repr(C)]` structs
/// of them. The kernel can copy a partial value before failing, and that
/// value is still dropped.
pub unsafe trait Plain: Default {}

unsafe impl Plain for i32 {}
unsafe impl Plain for u32 {}
unsafe impl Plain for i64 {}
unsafe impl Plain for u64 {}

/// Reads a fixed-size `sysctl` value:
///
/// ```
/// use top_metrics::macos::sys::sysctl_value;
/// assert!(sysctl_value::<u32>("hw.ncpu").is_some_and(|cpus| cpus > 0));
/// ```
///
/// The kernel writes raw bytes into `T`, so a type with pointers or
/// invariants is refused:
///
/// ```compile_fail,E0277
/// use top_metrics::macos::sys::sysctl_value;
/// let _ = sysctl_value::<String>("kern.ostype");
/// ```
pub fn sysctl_value<T: Plain>(name: &str) -> Option<T> {
    let name = CString::new(name).ok()?;
    let mut value = T::default();
    let mut len = size_of::<T>();
    let rc = unsafe {
        sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut T).cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && len == size_of::<T>()).then_some(value)
}

/// Reads a variable-size `sysctl` value through `read(oldp, oldlenp)`, growing
/// the buffer while the value races to outgrow it between the probe and the read.
fn read_growing(mut read: impl FnMut(*mut c_void, &mut usize) -> c_int) -> Option<Vec<u8>> {
    for _ in 0..4 {
        let mut len = 0usize;
        if read(ptr::null_mut(), &mut len) != 0 {
            return None;
        }
        len += len / 8;
        let mut buf = vec![0u8; len];
        if read(buf.as_mut_ptr().cast(), &mut len) == 0 {
            buf.truncate(len);
            return Some(buf);
        }
        if errno() != ENOMEM {
            return None;
        }
    }
    None
}

/// Reads a variable-size `sysctl` table.
pub fn sysctl_table(mib: &mut [c_int]) -> Option<Vec<u8>> {
    let namelen = mib.len() as c_uint;
    // `read_growing` passes a null `oldp` or one backed by `*oldlenp` bytes.
    read_growing(|oldp, len| unsafe {
        sysctl(mib.as_mut_ptr(), namelen, oldp, len, ptr::null_mut(), 0)
    })
}

/// Reads a string `sysctl` value.
pub fn sysctl_string(name: &str) -> Option<String> {
    let name = CString::new(name).ok()?;
    // `read_growing` passes a null `oldp` or one backed by `*oldlenp` bytes.
    let buf = read_growing(|oldp, len| unsafe {
        sysctlbyname(name.as_ptr(), oldp, len, ptr::null_mut(), 0)
    })?;
    Some(
        CStr::from_bytes_until_nul(&buf)
            .ok()?
            .to_string_lossy()
            .into_owned(),
    )
}

/// `struct xsw_usage` from <sys/sysctl.h>, the value of `vm.swapusage`.
#[repr(C)]
#[derive(Default)]
pub struct xsw_usage {
    pub xsu_total: u64,
    pub xsu_avail: u64,
    pub xsu_used: u64,
    pub xsu_pagesize: u32,
    pub xsu_encrypted: u32,
}

unsafe impl Plain for xsw_usage {}

/// `struct timeval` from <sys/_types/_timeval.h>, the value of `kern.boottime`.
#[repr(C)]
#[derive(Default)]
pub struct timeval {
    pub tv_sec: i64,
    pub tv_usec: i32,
}

unsafe impl Plain for timeval {}

/// Byte offsets into `struct kinfo_proc` (648 bytes on arm64).
pub mod kinfo_proc {
    pub const SIZE: usize = 648;
    /// `kp_proc.p_starttime`, a `timeval`.
    pub const START_SEC: usize = 0;
    pub const START_USEC: usize = 8;
    pub const PID: usize = 40;
    pub const COMM: usize = 243;
    pub const COMM_LEN: usize = 17;
    /// `kp_eproc.e_ucred.cr_uid`.
    pub const UID: usize = 420;
}

// <libproc.h>, <sys/proc_info.h>
pub const PROC_PIDTASKINFO: c_int = 4;
pub const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct proc_taskinfo {
    pub pti_virtual_size: u64,
    pub pti_resident_size: u64,
    /// Mach absolute time units, not nanoseconds, on Apple Silicon.
    pub pti_total_user: u64,
    pub pti_total_system: u64,
    pub pti_threads_user: u64,
    pub pti_threads_system: u64,
    pub pti_policy: i32,
    pub pti_faults: i32,
    pub pti_pageins: i32,
    pub pti_cow_faults: i32,
    pub pti_messages_sent: i32,
    pub pti_messages_received: i32,
    pub pti_syscalls_mach: i32,
    pub pti_syscalls_unix: i32,
    pub pti_csw: i32,
    pub pti_threadnum: i32,
    pub pti_numrunning: i32,
    pub pti_priority: i32,
}

unsafe extern "C" {
    pub fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    pub fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
}

// <notify.h>
unsafe extern "C" {
    pub fn notify_register_check(name: *const c_char, out_token: *mut c_int) -> u32;
    pub fn notify_get_state(token: c_int, state: *mut u64) -> u32;
    pub fn notify_cancel(token: c_int) -> u32;
}

// <pwd.h>, <signal.h>, <errno.h>
#[repr(C)]
pub struct passwd {
    pub pw_name: *mut c_char,
    pub pw_passwd: *mut c_char,
    pub pw_uid: u32,
    pub pw_gid: u32,
    pub pw_change: i64,
    pub pw_class: *mut c_char,
    pub pw_gecos: *mut c_char,
    pub pw_dir: *mut c_char,
    pub pw_shell: *mut c_char,
    pub pw_expire: i64,
}

pub const SIGTERM: c_int = 15;
pub const SIGKILL: c_int = 9;
pub const EPERM: c_int = 1;
pub const ESRCH: c_int = 3;
pub const EINVAL: c_int = 22;
pub const ERANGE: c_int = 34;

unsafe extern "C" {
    pub fn getpwuid_r(
        uid: u32,
        pwd: *mut passwd,
        buf: *mut c_char,
        buflen: usize,
        result: *mut *mut passwd,
    ) -> c_int;
    /// Private: callers go through [`send_signal`].
    fn kill(pid: c_int, sig: c_int) -> c_int;
    pub fn geteuid() -> u32;
    pub fn __error() -> *mut c_int;
}

pub fn errno() -> c_int {
    unsafe { *__error() }
}

/// Sends `sig` to the single process `pid`, or returns the errno. A pid of 0
/// would signal our own process group and -1 every process we may signal, so
/// both are refused with `EINVAL`. Guarding against pid reuse is the caller's job.
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

// <net/if.h>, <net/if_types.h>, <net/if_mib.h>
pub const IFF_LOOPBACK: u32 = 0x8;
pub const IFF_POINTOPOINT: u32 = 0x10;
pub const IFT_LOOP: u8 = 0x18;
pub const IFT_L2VLAN: u8 = 0x87;
pub const IFT_IEEE8023ADLAG: u8 = 0x88;
pub const IFT_BRIDGE: u8 = 0xd1;
pub const NETLINK_GENERIC: c_int = 0;
pub const IFMIB_IFALLDATA: c_int = 3;
pub const IFDATA_GENERAL: c_int = 1;

/// Byte offsets into `struct ifmibdata` (180 bytes). Its `struct if_data64`
/// is packed to 4 bytes, so x86_64 (Rosetta) has the same layout.
pub mod ifmibdata {
    pub const SIZE: usize = 180;
    /// `ifmd_name`, NUL-terminated within `IFNAMSIZ`; empty while the
    /// interface is still attaching or already detaching.
    pub const NAME: usize = 0;
    pub const NAME_LEN: usize = 16;
    /// `ifmd_flags`, the `IFF_*` flags.
    pub const FLAGS: usize = 20;
    /// `ifmd_data.ifi_type`, an `IFT_*` value.
    pub const TYPE: usize = 52;
    pub const MTU: usize = 52 + 8;
    pub const IBYTES: usize = 52 + 64;
    pub const OBYTES: usize = 52 + 72;
}

// <sys/mount.h>, in its 64-bit inode layout: the only one on arm64, and the
// `$INODE64` symbols on x86_64 (Rosetta).
pub const MFSNAMELEN: usize = 16;
pub const MAXPATHLEN: usize = 1024;
pub const MNT_NOWAIT: c_int = 2;
pub const MNT_LOCAL: u32 = 0x0000_1000;
pub const MNT_DONTBROWSE: u32 = 0x0010_0000;

#[repr(C)]
pub struct statfs {
    pub f_bsize: u32,
    pub f_iosize: i32,
    pub f_blocks: u64,
    pub f_bfree: u64,
    pub f_bavail: u64,
    pub f_files: u64,
    pub f_ffree: u64,
    pub f_fsid: [i32; 2],
    pub f_owner: u32,
    pub f_type: u32,
    pub f_flags: u32,
    pub f_fssubtype: u32,
    pub f_fstypename: [c_char; MFSNAMELEN],
    pub f_mntonname: [c_char; MAXPATHLEN],
    pub f_mntfromname: [c_char; MAXPATHLEN],
    pub f_flags_ext: u32,
    pub f_reserved: [u32; 7],
}

unsafe extern "C" {
    /// Thread-safe `getmntinfo`: the buffer belongs to the caller, who frees it.
    /// On x86_64 the plain symbol returns the old, smaller `statfs`.
    #[cfg_attr(target_arch = "x86_64", link_name = "getmntinfo_r_np$INODE64")]
    fn getmntinfo_r_np(mntbufp: *mut *mut statfs, flags: c_int) -> c_int;
    fn free(ptr: *mut c_void);
}

/// The kernel's mount table, freed on drop.
pub struct MountTable {
    entries: ptr::NonNull<statfs>,
    count: usize,
}

impl MountTable {
    /// `None` when the table can't be read. `MNT_NOWAIT` takes each
    /// filesystem's cached sizes, so an unresponsive network mount can't stall
    /// the read.
    pub fn read() -> Option<Self> {
        let mut entries: *mut statfs = ptr::null_mut();
        let count = unsafe { getmntinfo_r_np(&mut entries, MNT_NOWAIT) };
        let count = usize::try_from(count).ok().filter(|&count| count > 0)?;
        Some(Self {
            entries: ptr::NonNull::new(entries)?,
            count,
        })
    }

    pub fn entries(&self) -> &[statfs] {
        // `getmntinfo_r_np` filled `count` entries, which live until drop.
        unsafe { std::slice::from_raw_parts(self.entries.as_ptr(), self.count) }
    }
}

impl Drop for MountTable {
    fn drop(&mut self) {
        // `getmntinfo_r_np` mallocs the buffer and hands it to us.
        unsafe { free(self.entries.as_ptr().cast()) };
    }
}

/// Converts a fixed, NUL-padded C array to a `String`.
pub fn c_array_to_string(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Reads the native-endian integer at byte `at` of a record the kernel filled.
/// Panics past the end, so callers bound `at` by the record's size first.
pub fn read_i32(bytes: &[u8], at: usize) -> i32 {
    i32::from_ne_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

pub fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(bytes[at..at + 4].try_into().expect("4 bytes"))
}

pub fn read_i64(bytes: &[u8], at: usize) -> i64 {
    i64::from_ne_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
}

pub fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_ne_bytes(bytes[at..at + 8].try_into().expect("8 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_sysctl_reads_up_to_the_nul() {
        assert_eq!(sysctl_string("kern.ostype").as_deref(), Some("Darwin"));
    }

    #[test]
    fn missing_string_sysctl_is_none() {
        assert_eq!(sysctl_string("kern.no_such_name"), None);
    }

    #[test]
    fn table_sysctl_reads_this_process_at_the_kinfo_proc_offsets() {
        let pid = std::process::id() as c_int;
        let buf = sysctl_table(&mut [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid])
            .expect("kern.proc.pid unreadable");
        assert_eq!(buf.len(), kinfo_proc::SIZE);
        assert_eq!(read_i32(&buf, kinfo_proc::PID), pid);
        assert_eq!(read_i32(&buf, kinfo_proc::UID) as u32, unsafe { geteuid() });
    }

    #[test]
    fn interface_table_reads_loopback_at_the_ifmibdata_offsets() {
        let mut mib = [
            CTL_NET,
            PF_LINK,
            NETLINK_GENERIC,
            IFMIB_IFALLDATA,
            0,
            IFDATA_GENERAL,
        ];
        let table = sysctl_table(&mut mib).expect("net.link.generic.ifalldata unreadable");
        assert_eq!(table.len() % ifmibdata::SIZE, 0);
        let lo0 = table
            .chunks_exact(ifmibdata::SIZE)
            .find(|record| record[ifmibdata::NAME..].starts_with(b"lo0\0"))
            .expect("no lo0 record");
        assert_ne!(read_u32(lo0, ifmibdata::FLAGS) & IFF_LOOPBACK, 0);
        assert_eq!(lo0[ifmibdata::TYPE], IFT_LOOP);
        // xnu's `LOMTU`: a wrong `if_data64` offset lands this read elsewhere.
        assert_eq!(read_u32(lo0, ifmibdata::MTU), 16384);
    }

    #[test]
    fn mount_table_entries_read_back_as_mount_paths() {
        let table = MountTable::read().expect("getmntinfo_r_np failed");
        let paths: Vec<String> = table
            .entries()
            .iter()
            .map(|mount| c_array_to_string(&mount.f_mntonname))
            .collect();
        // A wrong `statfs` layout lands these reads mid-entry, off the paths.
        assert!(paths.iter().any(|path| path == "/"), "{paths:?}");
        assert!(paths.iter().all(|path| path.starts_with('/')), "{paths:?}");
    }

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
}
