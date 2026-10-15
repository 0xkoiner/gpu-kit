//! The pipeline's data types. Their behaviour lives beside them, in
//! `history.rs`, `reading.rs` and the module root.

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                       Tracker Structs                        */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use super::constants::{
    BUSIEST_LEN, DEFAULT_INTERVAL, HISTORY_LEN, MAX_HISTORY_LEN, MAX_INTERVAL, MIN_INTERVAL,
};
use crate::snapshot::Snapshot;
use crate::tracker::worker::Shared;

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                           Structs                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

/// How a [`Tracker`](crate::tracker::Tracker) samples. Fill the fields that
/// matter over the defaults: `TrackerConfig { interval, ..Default::default() }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackerConfig {
    /// The wait between samples, clamped to `MIN_INTERVAL..=MAX_INTERVAL`.
    pub interval: Duration,
    /// Samples each series keeps, clamped to `1..=MAX_HISTORY_LEN`. The
    /// history is cloned into every reading, so keep it to what a chart draws.
    pub history_len: usize,
    /// Processes a reading ranks by CPU; 0 ranks none.
    pub busiest_len: usize,
}

/// One metric's recent values, oldest first, with `None` where a sample
/// lacked the metric, so a gap in the data stays a gap in the chart.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Series {
    pub(crate) values: VecDeque<Option<f32>>,
    pub(crate) capacity: usize,
    /// The largest value ever pushed, which outlives the window.
    pub(crate) peak: Option<f32>,
}

/// Recent values of every [`Metric`](super::enums::Metric), each with its
/// peak since the tracker started.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct History {
    pub(crate) cpu: Series,
    pub(crate) gpu: Series,
    pub(crate) memory: Series,
    pub(crate) power: Series,
    pub(crate) network_receive: Series,
    pub(crate) network_send: Series,
    pub(crate) disk_read: Series,
    pub(crate) disk_write: Series,
}

/// What the tracker publishes after each sample: the snapshot and what every
/// surface derives from it, computed once on the sampling thread.
#[derive(Debug, Clone)]
pub struct Reading {
    /// 1 for the first published reading, one more per publish.
    pub(crate) sequence: u64,
    pub(crate) taken_at: SystemTime,
    pub(crate) snapshot: Snapshot,
    /// Indices into `snapshot.processes`, busiest first.
    pub(crate) busiest: Vec<usize>,
    pub(crate) history: History,
}

/// Keeps processes listed while alive; see
/// [`Tracker::demand_processes`](crate::tracker::Tracker::demand_processes).
#[must_use = "processes are listed only while the guard lives"]
pub struct ProcessDemand(pub(crate) Arc<Shared>);

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                          Functions                           */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            interval: DEFAULT_INTERVAL,
            history_len: HISTORY_LEN,
            busiest_len: BUSIEST_LEN,
        }
    }
}

impl TrackerConfig {
    /// The config within the bounds the loop relies on.
    pub(crate) fn normalized(self) -> Self {
        Self {
            interval: self.interval.clamp(MIN_INTERVAL, MAX_INTERVAL),
            history_len: self.history_len.clamp(1, MAX_HISTORY_LEN),
            busiest_len: self.busiest_len,
        }
    }
}

impl fmt::Debug for ProcessDemand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProcessDemand")
    }
}
