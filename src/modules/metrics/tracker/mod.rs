//! The continuous pipeline over [`Sampler`]: one background thread samples at
//! a steady interval and publishes an immutable [`Reading`] after each sample,
//! the snapshot plus its history, peaks and busiest processes, computed once.
//! Consumers pull [`Tracker::latest`] or receive every reading from
//! [`Tracker::subscribe`]; nothing here renders or formats.

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                           Tracker                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

mod history;
mod reading;
pub mod types;
mod worker;

#[cfg(test)]
#[path = "test/tests.rs"]
mod tests;

use std::io;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use self::types::constants::THREAD_NAME;
pub use self::types::enums::{Metric, Unit};
pub use self::types::structs::{History, ProcessDemand, Reading, Series, TrackerConfig};
use self::worker::{Shared, locked};
use crate::Sampler;
use crate::snapshot::ChipInfo;

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                           Structs                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

/// Samples the machine on a background thread and publishes a [`Reading`]
/// after each sample. Every method takes `&self`, so share it behind an `Arc`.
pub struct Tracker {
    chip: ChipInfo,
    shared: Arc<Shared>,
    /// Taken by `stop`, which joins it.
    thread: Mutex<Option<JoinHandle<()>>>,
}

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                          Functions                           */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

impl Tracker {
    /// Builds the sampler on this thread (the GPU library, RAPL and the rate
    /// baselines, so a moment) and starts sampling. `Err` only when the OS
    /// refuses a thread.
    pub fn start(config: TrackerConfig) -> io::Result<Self> {
        let config = config.normalized();
        let sampler = Sampler::new();
        let chip = sampler.chip().clone();
        let shared = Arc::new(Shared::new(config.interval));
        let worker = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || worker::run(sampler, worker, config))?;
        Ok(Self {
            chip,
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Static facts about the machine, known from the start.
    pub fn chip(&self) -> &ChipInfo {
        &self.chip
    }

    /// The newest reading; `None` until the first interval has elapsed.
    pub fn latest(&self) -> Option<Arc<Reading>> {
        self.shared.latest()
    }

    /// Every reading published from now on, in order. A consumer that falls
    /// `UPDATE_QUEUE` readings behind misses some rather than stalling the
    /// sampler; dropping the receiver unsubscribes.
    pub fn subscribe(&self) -> Receiver<Arc<Reading>> {
        self.shared.subscribe()
    }

    /// The wait between samples.
    pub fn interval(&self) -> Duration {
        self.shared.interval()
    }

    /// Sets the wait, clamped to `MIN_INTERVAL..=MAX_INTERVAL`; it applies
    /// from the next tick.
    pub fn set_interval(&self, interval: Duration) {
        self.shared.set_interval(interval);
    }

    pub fn is_frozen(&self) -> bool {
        self.shared.is_frozen()
    }

    /// Frozen, the thread keeps sampling so rates stay fresh but publishes
    /// nothing: `latest`, history, peaks and `sequence` stand still until
    /// thawed.
    pub fn set_frozen(&self, frozen: bool) {
        self.shared.set_frozen(frozen);
    }

    /// Keeps processes listed while the guard lives. The first guard wakes
    /// the thread, which records a baseline, waits `PROCESS_BASELINE` and
    /// samples; once the last guard drops, readings carry `processes: None`.
    pub fn demand_processes(&self) -> ProcessDemand {
        if self.shared.add_demand() == 0 {
            self.shared.wake();
        }
        ProcessDemand(Arc::clone(&self.shared))
    }

    /// Whether a [`ProcessDemand`] is alive.
    pub fn lists_processes(&self) -> bool {
        self.shared.demand() > 0
    }

    /// Whether the sampling thread is still running.
    pub fn is_alive(&self) -> bool {
        locked(&self.thread)
            .as_ref()
            .is_some_and(|thread| !thread.is_finished())
    }

    /// Ends sampling and waits for the thread, which leaves its current wait
    /// at once and its current sample within milliseconds.
    pub fn stop(&self) {
        self.shared.stop();
        if let Some(thread) = locked(&self.thread).take() {
            // A sampler that panicked has nothing left to report.
            let _ = thread.join();
        }
    }

    /// Live subscriber queues: an observation seam for the tests.
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.shared.subscriber_count()
    }
}

impl Drop for Tracker {
    /// Signals only: the thread exits by itself within one wait, so dropping
    /// a tracker never blocks.
    fn drop(&mut self) {
        self.shared.stop();
    }
}

impl Drop for ProcessDemand {
    fn drop(&mut self) {
        self.0.release_demand();
    }
}
