use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_int};
use std::time::Instant;

use super::cf;
use super::sys::{self, read_i32, read_i64};
use crate::signal::{Signal, SignalError};
use crate::snapshot::Process;

/// A pid is reused after exit; the start time tells the two processes apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ProcessKey {
    pid: i32,
    start_sec: i64,
    start_usec: i32,
}

impl ProcessKey {
    fn read(entry: &[u8]) -> Self {
        Self {
            pid: read_i32(entry, sys::kinfo_proc::PID),
            start_sec: read_i64(entry, sys::kinfo_proc::START_SEC),
            start_usec: read_i32(entry, sys::kinfo_proc::START_USEC),
        }
    }

    /// Start time in microseconds since the epoch.
    fn started_at(self) -> u64 {
        self.start_sec.max(0) as u64 * 1_000_000 + self.start_usec.max(0) as u64
    }
}

struct Previous {
    cpu_ns: u64,
}

/// `p_comm`: the start of the executable's file name, NUL-padded.
type Comm = [u8; sys::kinfo_proc::COMM_LEN];

/// A process's display name and the `p_comm` it was resolved under. `exec`
/// keeps the pid and start time but renames `p_comm`, so a changed `p_comm`
/// means the name is stale.
struct Name {
    comm: Comm,
    shown: String,
}

/// A GPU user client: its owner's pid and its registry entry ID, which isn't
/// reused while the system is up.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GpuClient {
    pid: i32,
    id: u64,
}

/// Lists every process and turns cumulative CPU and GPU time into rates.
pub struct ProcessTable {
    timebase: sys::mach_timebase_info,
    /// Task info is readable only for our own processes, unless we run as root.
    euid: u32,
    previous: HashMap<ProcessKey, Previous>,
    previous_gpu: HashMap<GpuClient, u64>,
    previous_at: Option<Instant>,
    users: HashMap<u32, String>,
    names: HashMap<ProcessKey, Name>,
}

impl ProcessTable {
    pub fn new() -> Self {
        let mut timebase = sys::mach_timebase_info::default();
        unsafe { sys::mach_timebase_info(&mut timebase) };
        if timebase.denom == 0 {
            timebase = sys::mach_timebase_info { numer: 1, denom: 1 };
        }
        Self {
            timebase,
            euid: unsafe { sys::geteuid() },
            previous: HashMap::new(),
            previous_gpu: HashMap::new(),
            previous_at: None,
            users: HashMap::new(),
            names: HashMap::new(),
        }
    }

    /// Every process, with its CPU and GPU use since the last sample. `None`
    /// when the process table can't be read; the baselines are kept, so the
    /// next sample spans the gap.
    pub fn sample(&mut self, memory_total: u64) -> Option<Vec<Process>> {
        let mut mib = [sys::CTL_KERN, sys::KERN_PROC, sys::KERN_PROC_ALL];
        let table = sys::sysctl_table(&mut mib)?;
        let now = Instant::now();
        let wall_ns = self
            .previous_at
            .map(|at| now.duration_since(at).as_nanos() as f64)
            .filter(|&ns| ns > 0.0);
        self.previous_at = Some(now);

        let gpu_now = gpu_time_by_client();
        let gpu_used = gpu_ns_by_pid(&self.previous_gpu, &gpu_now);
        let mut previous = HashMap::with_capacity(self.previous.len());
        let mut names = HashMap::with_capacity(self.names.len());
        let mut processes = Vec::new();

        for entry in table.chunks_exact(sys::kinfo_proc::SIZE) {
            let key = ProcessKey::read(entry);
            if key.pid == 0 {
                continue;
            }
            let uid = read_i32(entry, sys::kinfo_proc::UID) as u32;
            let comm: Comm = entry[sys::kinfo_proc::COMM..][..sys::kinfo_proc::COMM_LEN]
                .try_into()
                .expect("COMM_LEN bytes");
            let name = match self.names.remove(&key) {
                Some(name) if name.comm == comm => name,
                _ => Name {
                    comm,
                    shown: command_name(key.pid, &comm),
                },
            };
            let user = self.user_name(uid);

            let mut process = Process {
                pid: key.pid,
                started_at: key.started_at(),
                user,
                command: name.shown.clone(),
                ..Process::default()
            };

            // Other users' processes refuse task info without root, so don't ask; they keep zeros.
            let readable = self.euid == 0 || uid == self.euid;
            if let Some(task) = readable.then(|| task_info(key.pid)).flatten() {
                let cpu_ns = self.ticks_to_ns(task.pti_total_user + task.pti_total_system);
                if let (Some(wall_ns), Some(before)) = (wall_ns, self.previous.get(&key))
                    && cpu_ns >= before.cpu_ns
                {
                    process.cpu = ((cpu_ns - before.cpu_ns) as f64 / wall_ns * 100.0) as f32;
                }
                process.cpu_time = cpu_ns as f64 / 1e9;
                process.resident = task.pti_resident_size;
                if memory_total > 0 {
                    process.memory =
                        (task.pti_resident_size as f64 / memory_total as f64 * 100.0) as f32;
                }
                previous.insert(key, Previous { cpu_ns });
            }

            if let (Some(wall_ns), Some(&gpu_ns)) = (wall_ns, gpu_used.get(&key.pid)) {
                process.gpu = (gpu_ns as f64 / wall_ns * 100.0).clamp(0.0, 100.0) as f32;
            }

            names.insert(key, name);
            processes.push(process);
        }

        self.previous = previous;
        self.previous_gpu = gpu_now;
        self.names = names;
        Some(processes)
    }

    fn ticks_to_ns(&self, ticks: u64) -> u64 {
        (ticks as u128 * self.timebase.numer as u128 / self.timebase.denom as u128) as u64
    }

    fn user_name(&mut self, uid: u32) -> String {
        self.users
            .entry(uid)
            .or_insert_with(|| lookup_user(uid))
            .clone()
    }
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Signals the process `pid` only while it is still the one that started at
/// `started_at` (see [`Process::started_at`]), so a reused pid is never hit.
pub fn send_signal(pid: i32, started_at: u64, signal: Signal) -> Result<(), SignalError> {
    if !is_running(pid, started_at) {
        return Err(SignalError::NoSuchProcess);
    }
    let sig = match signal {
        Signal::Terminate => sys::SIGTERM,
        Signal::Kill => sys::SIGKILL,
    };
    sys::send_signal(pid, sig).map_err(|errno| match errno {
        sys::EPERM => SignalError::NotPermitted,
        sys::ESRCH => SignalError::NoSuchProcess,
        other => SignalError::Other(other),
    })
}

/// Whether the process that started at `started_at` is still running as `pid`.
/// Never for a pid of 0 or below: `kern.proc.pid.0` reads `kernel_task`, but
/// signalling pid 0 would hit our own process group.
pub fn is_running(pid: i32, started_at: u64) -> bool {
    let mut mib = [sys::CTL_KERN, sys::KERN_PROC, sys::KERN_PROC_PID, pid];
    pid > 0
        && sys::sysctl_table(&mut mib).is_some_and(|entry| {
            entry.len() == sys::kinfo_proc::SIZE
                && ProcessKey::read(&entry).started_at() == started_at
        })
}

fn task_info(pid: c_int) -> Option<sys::proc_taskinfo> {
    let mut info = sys::proc_taskinfo::default();
    let size = size_of::<sys::proc_taskinfo>() as c_int;
    let read = unsafe {
        sys::proc_pidinfo(
            pid,
            sys::PROC_PIDTASKINFO,
            0,
            (&mut info as *mut sys::proc_taskinfo).cast(),
            size,
        )
    };
    (read == size).then_some(info)
}

/// The executable's file name, or `comm` when its path can't be read.
fn command_name(pid: c_int, comm: &Comm) -> String {
    let mut path = vec![0u8; sys::PROC_PIDPATHINFO_MAXSIZE];
    let len = unsafe { sys::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
    if len > 0 {
        let path = String::from_utf8_lossy(&path[..len as usize]);
        let name = displayable(path.rsplit('/').next().unwrap_or_default());
        if !name.is_empty() {
            return name;
        }
    }
    let comm = CStr::from_bytes_until_nul(comm).map_or(&comm[..], CStr::to_bytes);
    displayable(&String::from_utf8_lossy(comm))
}

/// `name` without control or bidi control characters, which any process can
/// put in its own executable's name to break or reorder the row showing it.
fn displayable(name: &str) -> String {
    name.chars()
        .filter(|&c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                )
        })
        .collect()
}

fn lookup_user(uid: u32) -> String {
    let mut pwd: sys::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut sys::passwd = std::ptr::null_mut();
    // Directory-service accounts can outgrow 1 KiB, so grow the buffer on `ERANGE`.
    let mut size = 1024;
    loop {
        let mut buf = vec![0 as c_char; size];
        let rc =
            unsafe { sys::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
        if rc == sys::ERANGE && size < 1 << 20 {
            size *= 2;
            continue;
        }
        if rc == 0 && !result.is_null() && !pwd.pw_name.is_null() {
            // `pw_name` points into `buf`, which is still alive.
            return unsafe { CStr::from_ptr(pwd.pw_name) }
                .to_string_lossy()
                .into_owned();
        }
        return uid.to_string();
    }
}

/// Cumulative GPU time in nanoseconds of each user client of the GPU driver.
fn gpu_time_by_client() -> HashMap<GpuClient, u64> {
    let mut clients = HashMap::new();
    cf::for_each_service("AGXAccelerator", |accelerator| {
        cf::for_each_child(accelerator, |client| {
            let Some(pid) = cf::property(client, "IOUserClientCreator")
                .and_then(|creator| cf::string(&creator))
                .and_then(|creator| parse_creator_pid(&creator))
            else {
                return;
            };
            let (Some(id), Some(usage)) =
                (cf::registry_id(client), cf::property(client, "AppUsage"))
            else {
                return;
            };
            let ns: u64 = cf::array_items(&usage)
                .filter_map(|item| cf::dict_get(item, "accumulatedGPUTime"))
                .filter_map(cf::number_i64)
                .map(|ns| ns.max(0) as u64)
                .sum();
            clients.insert(GpuClient { pid, id }, ns);
        });
    });
    clients
}

/// GPU nanoseconds each pid used between two reads, counting only clients in
/// both, so one that closes can't cancel out the others and one that opens
/// (or a reused pid's) can't pass its total off as the interval's.
fn gpu_ns_by_pid(
    before: &HashMap<GpuClient, u64>,
    after: &HashMap<GpuClient, u64>,
) -> HashMap<i32, u64> {
    let mut used = HashMap::new();
    for (client, &ns) in after {
        if let Some(&before) = before.get(client) {
            let total: &mut u64 = used.entry(client.pid).or_default();
            *total = total.saturating_add(ns.saturating_sub(before));
        }
    }
    used
}

/// Parses `"pid 454, WindowServer"`.
fn parse_creator_pid(creator: &str) -> Option<i32> {
    creator
        .strip_prefix("pid ")?
        .split(',')
        .next()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use super::*;

    #[test]
    fn parses_user_client_creator() {
        assert_eq!(parse_creator_pid("pid 454, WindowServer"), Some(454));
        assert_eq!(parse_creator_pid("WindowServer"), None);
    }

    #[test]
    fn gpu_time_counts_only_clients_in_both_reads() {
        let client = |pid, id| GpuClient { pid, id };
        let before = HashMap::from([
            (client(454, 1), 1_000),
            (client(454, 2), 5_000),
            (client(99, 3), 700),
        ]);
        // Client 2 closed; pid 99 exited and went to a process whose client 4 is new.
        let after = HashMap::from([(client(454, 1), 1_600), (client(99, 4), 9_000)]);
        assert_eq!(gpu_ns_by_pid(&before, &after), HashMap::from([(454, 600)]));
    }

    #[test]
    fn names_drop_characters_that_break_or_reorder_a_row() {
        assert_eq!(displayable("evil\u{202E}gpj.app"), "evilgpj.app");
        assert_eq!(displayable("two\nlines\u{7}"), "twolines");
        assert_eq!(displayable("Code Helper (GPU)"), "Code Helper (GPU)");
    }

    #[test]
    fn recognises_a_process_only_by_its_start_time() {
        let mut table = ProcessTable::new();
        let me = table
            .sample(1 << 34)
            .expect("process table")
            .into_iter()
            .find(|p| p.pid == std::process::id() as i32)
            .expect("current process listed");
        assert!(is_running(me.pid, me.started_at));
        assert!(
            !is_running(me.pid, me.started_at + 1),
            "a reused pid has another start time"
        );
        assert_eq!(
            send_signal(me.pid, me.started_at + 1, Signal::Terminate),
            Err(SignalError::NoSuchProcess)
        );
    }

    #[test]
    fn kernel_task_is_never_a_signal_target() {
        // `kern.proc.pid.0` reads `kernel_task`, start time and all, but
        // `kill(0, …)` would signal our own process group.
        let mut mib = [sys::CTL_KERN, sys::KERN_PROC, sys::KERN_PROC_PID, 0];
        let kernel = sys::sysctl_table(&mut mib).expect("kern.proc.pid.0 unreadable");
        assert_eq!(kernel.len(), sys::kinfo_proc::SIZE);
        assert!(!is_running(0, ProcessKey::read(&kernel).started_at()));
    }

    #[test]
    fn lists_the_current_process_with_its_own_task_info() {
        let mut table = ProcessTable::new();
        let processes = table.sample(1 << 34).expect("process table");
        let me = processes
            .iter()
            .find(|p| p.pid == std::process::id() as i32)
            .expect("current process listed");
        assert!(me.resident > 0);
        assert!(!me.command.is_empty());
    }

    #[test]
    fn a_process_is_renamed_when_it_execs() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "read line && exec /bin/sleep 30"])
            .stdin(Stdio::piped())
            .spawn()
            .expect("spawn sh");
        let pid = child.id() as i32;
        let mut table = ProcessTable::new();
        let mut command = || {
            table
                .sample(1 << 34)
                .expect("process table")
                .into_iter()
                .find(|p| p.pid == pid)
                .map(|p| p.command)
        };
        // Caches the shell's name under the pid and start time `exec` keeps.
        assert_ne!(command().expect("child listed"), "sleep");
        let mut stdin = child.stdin.take().expect("piped stdin");
        stdin.write_all(b"\n").expect("wake the shell");
        drop(stdin);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut renamed = false;
        while !renamed && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            renamed = command().as_deref() == Some("sleep");
        }
        let _ = child.kill();
        let _ = child.wait();
        assert!(renamed, "the name from before exec stuck");
    }
}
