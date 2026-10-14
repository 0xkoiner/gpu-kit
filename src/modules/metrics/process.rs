//! Every process from /proc, with CPU and GPU use since the last sample.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Instant;

use crate::snapshot::Process;
use crate::{files, sys};

/// A pid is reused after exit; the start time tells the two processes apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ProcessKey {
    pid: i32,
    started_at: u64,
}

/// What /proc/<pid>/stat says of a process.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stat {
    comm: String,
    /// User plus system time, in clock ticks.
    cpu_ticks: u64,
    /// Clock ticks after boot.
    start_ticks: u64,
    rss_pages: u64,
}

/// Lists every process and turns cumulative CPU time into rates.
pub struct ProcessTable {
    clk_tck: u64,
    page_size: u64,
    /// Each process's CPU ticks at the last sample.
    previous: HashMap<ProcessKey, u64>,
    previous_at: Option<Instant>,
    users: HashMap<u32, String>,
}

impl ProcessTable {
    pub fn new() -> Self {
        Self {
            clk_tck: clk_tck(),
            page_size: sys::sysconf_value(sys::_SC_PAGESIZE)
                .filter(|&size| size > 0)
                .unwrap_or(4096),
            previous: HashMap::new(),
            previous_at: None,
            users: read_users().unwrap_or_default(),
        }
    }

    /// Every process, with its CPU use since the last sample and, from
    /// `gpu_shares`, its share of the GPU. `None` when /proc can't be listed
    /// or the boot time read; the baselines are kept, so the next sample
    /// spans the gap.
    pub fn sample(
        &mut self,
        memory_total: u64,
        gpu_shares: &HashMap<i32, f32>,
    ) -> Option<Vec<Process>> {
        let pids = pids(Path::new("/proc"))?;
        let boot_secs = boot_time()?;
        let now = Instant::now();
        let wall_secs = self
            .previous_at
            .map(|at| now.duration_since(at).as_secs_f64().max(1e-3));
        let mut current = HashMap::with_capacity(pids.len());
        let mut reloaded_users = false;
        let processes = pids
            .into_iter()
            .filter_map(|pid| {
                // A process that exited since /proc was listed has no files left.
                let stat = parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)?;
                let key = ProcessKey {
                    pid,
                    started_at: start_micros(boot_secs, self.clk_tck, stat.start_ticks),
                };
                current.insert(key, stat.cpu_ticks);
                let cpu = match (self.previous.get(&key), wall_secs) {
                    (Some(&before), Some(secs)) => {
                        let busy =
                            stat.cpu_ticks.saturating_sub(before) as f64 / self.clk_tck as f64;
                        (busy / secs * 100.0) as f32
                    }
                    _ => 0.0,
                };
                let resident = stat.rss_pages.saturating_mul(self.page_size);
                let memory = if memory_total > 0 {
                    (resident as f64 / memory_total as f64 * 100.0) as f32
                } else {
                    0.0
                };
                // The process's directory belongs to its user.
                let user = fs::metadata(format!("/proc/{pid}"))
                    .ok()
                    .map(|dir| user_name(&mut self.users, &mut reloaded_users, dir.uid()))
                    .unwrap_or_default();
                Some(Process {
                    pid,
                    started_at: key.started_at,
                    user,
                    command: command(pid, &stat.comm),
                    cpu,
                    gpu: gpu_shares.get(&pid).copied().unwrap_or(0.0),
                    memory,
                    resident,
                    cpu_time: stat.cpu_ticks as f64 / self.clk_tck as f64,
                })
            })
            .collect();
        self.previous = current;
        self.previous_at = Some(now);
        Some(processes)
    }
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self::new()
    }
}

/// The pids under /proc: every entry with a numeric name. `None` when /proc
/// can't be listed.
fn pids(proc: &Path) -> Option<Vec<i32>> {
    let entries = fs::read_dir(proc).ok()?;
    Some(
        entries
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .collect(),
    )
}

/// /proc/<pid>/stat. `comm` sits in parentheses and may itself hold spaces
/// or parentheses, so the fields are split after the last one. From there,
/// proc(5) numbers them from 3 (the state): utime is 14, stime 15, starttime
/// 22 and rss 24.
fn parse_stat(stat: &str) -> Option<Stat> {
    let (head, tail) = stat.rsplit_once(')')?;
    let (_, comm) = head.split_once('(')?;
    let fields: Vec<&str> = tail.split_whitespace().collect();
    let field = |number: usize| fields.get(number - 3)?.parse::<i64>().ok();
    let count = |value: i64| u64::try_from(value).ok();
    Some(Stat {
        comm: comm.to_owned(),
        cpu_ticks: count(field(14)?)?.saturating_add(count(field(15)?)?),
        start_ticks: count(field(22)?)?,
        // Printed signed, though never negative; a zombie's reads 0.
        rss_pages: count(field(24)?).unwrap_or(0),
    })
}

/// Clock ticks per second, the unit of /proc's CPU times: 100 on every Linux
/// build so far, but the kernel is asked.
fn clk_tck() -> u64 {
    sys::sysconf_value(sys::_SC_CLK_TCK)
        .filter(|&ticks| ticks > 0)
        .unwrap_or(100)
}

/// Microseconds since the epoch at which a process started, from the clock
/// ticks after boot /proc counts and the boot time in seconds.
fn start_micros(boot_secs: u64, clk_tck: u64, start_ticks: u64) -> u64 {
    let since_boot = u128::from(start_ticks) * 1_000_000 / u128::from(clk_tck);
    u64::try_from(u128::from(boot_secs) * 1_000_000 + since_boot).unwrap_or(u64::MAX)
}

/// /proc/stat's `btime`, in seconds since the epoch, read once: the kernel
/// derives it from the uptime and it can move a second between reads, which
/// would change every process's `started_at`, and so its identity.
fn boot_time() -> Option<u64> {
    static BOOT: OnceLock<u64> = OnceLock::new();
    if let Some(&secs) = BOOT.get() {
        return Some(secs);
    }
    let secs = files::read("/proc/stat")?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    Some(*BOOT.get_or_init(|| secs))
}

/// The program's name: its first argument's file name, or for a kernel
/// thread, whose command line is empty, the `comm` /proc/<pid>/stat holds.
fn command(pid: i32, comm: &str) -> String {
    fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .and_then(|cmdline| command_name(&cmdline))
        .unwrap_or_else(|| comm.to_owned())
}

/// The file name of the first NUL-separated argument.
fn command_name(cmdline: &[u8]) -> Option<String> {
    let first = cmdline.split(|&byte| byte == 0).next()?;
    let name = first.rsplit(|&byte| byte == b'/').next()?;
    (!name.is_empty()).then(|| String::from_utf8_lossy(name).into_owned())
}

/// The user `uid` names, or the number itself. An unknown uid re-reads
/// /etc/passwd once per sample, for a user added since the table was built.
fn user_name(users: &mut HashMap<u32, String>, reloaded: &mut bool, uid: u32) -> String {
    if !users.contains_key(&uid) && !*reloaded {
        *reloaded = true;
        if let Some(fresh) = read_users() {
            *users = fresh;
        }
    }
    users.get(&uid).cloned().unwrap_or_else(|| uid.to_string())
}

/// Names by uid from /etc/passwd. Users a directory service (LDAP, SSSD)
/// provides aren't in it and show as numbers.
fn read_users() -> Option<HashMap<u32, String>> {
    Some(parse_passwd(&fs::read_to_string("/etc/passwd").ok()?))
}

fn parse_passwd(passwd: &str) -> HashMap<u32, String> {
    passwd
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let uid = fields.nth(1)?.parse().ok()?;
            Some((uid, name.to_owned()))
        })
        .collect()
}

/// Why a signal could not be delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalError {
    /// The process belongs to another user or to the system.
    NotPermitted,
    /// The process already exited.
    NoSuchProcess,
    Other(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Terminate,
    Kill,
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

/// Whether the process that started at `started_at` is still running as
/// `pid`. Never for a pid of 0 or below, which name process groups rather
/// than a process.
pub fn is_running(pid: i32, started_at: u64) -> bool {
    pid > 0
        && fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| parse_stat(&stat))
            .zip(boot_time())
            .is_some_and(|(stat, boot_secs)| {
                start_micros(boot_secs, clk_tck(), stat.start_ticks) == started_at
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_fields_are_read_past_a_comm_with_spaces_and_parentheses() {
        let stat = "1234 (a (b) c) S 1 1234 1234 0 -1 4194560 1911 0 0 0 25 188 0 0 20 0 7 0 23 23339008 1289 18446744073709551615 1 1 0 0";
        assert_eq!(
            parse_stat(stat),
            Some(Stat {
                comm: "a (b) c".to_owned(),
                cpu_ticks: 25 + 188,
                start_ticks: 23,
                rss_pages: 1289,
            })
        );
        assert!(parse_stat("1234 (short) S 1").is_none());
    }

    #[test]
    fn start_time_is_the_boot_time_plus_the_ticks_since() {
        assert_eq!(
            start_micros(1_791_984_757, 100, 23),
            1_791_984_757_000_000 + 230_000
        );
        assert_eq!(start_micros(u64::MAX, 100, 1), u64::MAX);
    }

    #[test]
    fn passwd_maps_uids_to_names() {
        let users = parse_passwd(
            "root:x:0:0:root:/root:/bin/bash\nme:x:1000:1000::/home/me:/bin/sh\nbroken line\n",
        );
        assert_eq!(users.get(&0).map(String::as_str), Some("root"));
        assert_eq!(users.get(&1000).map(String::as_str), Some("me"));
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn the_command_is_the_first_argument_s_file_name() {
        assert_eq!(
            command_name(b"/usr/bin/python3\0-m\0http.server\0").as_deref(),
            Some("python3")
        );
        assert_eq!(command_name(b"sshd: me@pts/0\0").as_deref(), Some("0"));
        assert_eq!(command_name(b""), None, "a kernel thread");
    }

    #[test]
    fn our_own_process_is_listed_and_running() {
        let mut table = ProcessTable::new();
        let me = std::process::id() as i32;
        let first = table.sample(1 << 34, &HashMap::new()).expect("/proc");
        let process = first.iter().find(|p| p.pid == me).expect("ourselves");
        assert!(process.resident > 0 && process.memory > 0.0);
        assert!(!process.user.is_empty() && !process.command.is_empty());
        assert!(is_running(me, process.started_at));
        assert!(!is_running(me, process.started_at + 1));
        assert_eq!(
            send_signal(me, process.started_at + 1, Signal::Terminate),
            Err(SignalError::NoSuchProcess)
        );
        // Busy-wait a little so the second sample has CPU time to report.
        let spin = Instant::now();
        while spin.elapsed().as_millis() < 20 {}
        let second = table.sample(1 << 34, &HashMap::new()).expect("/proc");
        let process = second.iter().find(|p| p.pid == me).expect("ourselves");
        assert!(process.cpu > 0.0, "{process:?}");
    }
}
