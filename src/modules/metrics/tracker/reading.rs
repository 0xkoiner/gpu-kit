//! The unit the tracker publishes, with its process ranking.

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                           Reading                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

use std::time::SystemTime;

use super::types::structs::{History, Reading};
use crate::snapshot::{Process, Snapshot};

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                          Functions                           */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

impl Reading {
    /// Ranks the busiest processes and stamps the time, on the sampling thread.
    pub(crate) fn new(
        sequence: u64,
        snapshot: Snapshot,
        history: History,
        busiest_len: usize,
    ) -> Self {
        let busiest = rank_busiest(snapshot.processes.as_deref().unwrap_or(&[]), busiest_len);
        Self {
            sequence,
            taken_at: SystemTime::now(),
            snapshot,
            busiest,
            history,
        }
    }

    /// 1 for the first published reading, one more per publish; it stands
    /// still while the tracker is frozen.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// When the sample was taken.
    pub fn taken_at(&self) -> SystemTime {
        self.taken_at
    }

    /// The measurements of this interval.
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Every metric's recent values, up to and including this snapshot's.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// The processes with the highest CPU share, highest first; empty while
    /// processes aren't listed.
    pub fn busiest(&self) -> impl Iterator<Item = &Process> + '_ {
        let processes = self.snapshot.processes.as_deref().unwrap_or(&[]);
        self.busiest.iter().filter_map(move |&ix| processes.get(ix))
    }
}

/// Indices of the `count` processes with the highest CPU share, highest
/// first. A NaN share ranks as idle, and an index is never trusted blindly.
fn rank_busiest(processes: &[Process], count: usize) -> Vec<usize> {
    if count == 0 {
        return Vec::new();
    }
    let share = |ix: &usize| {
        processes
            .get(*ix)
            .map(|process| process.cpu)
            .filter(|cpu| !cpu.is_nan())
            .unwrap_or(0.0)
    };
    let by_share = |a: &usize, b: &usize| share(b).total_cmp(&share(a));
    let mut ranked: Vec<usize> = (0..processes.len()).collect();
    if ranked.len() > count {
        ranked.select_nth_unstable_by(count, by_share);
        ranked.truncate(count);
    }
    ranked.sort_by(by_share);
    ranked
}
