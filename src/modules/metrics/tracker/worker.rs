//! The sampling thread: its loop, and the control block it shares with the
//! tracker.

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                            Worker                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::types::constants::{MAX_INTERVAL, MIN_INTERVAL, PROCESS_BASELINE, UPDATE_QUEUE};
use super::types::structs::{History, Reading, TrackerConfig};
use crate::Sampler;
use crate::snapshot::Snapshot;

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                           Structs                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

/// What the tracker and its thread share: the controls, the demand for
/// processes, the latest reading and the subscribers.
pub(crate) struct Shared {
    control: Mutex<Control>,
    /// Signalled on every control change the loop must see at once.
    wake: Condvar,
    /// Live [`ProcessDemand`](super::ProcessDemand) guards.
    demand: AtomicUsize,
    latest: Mutex<Option<Arc<Reading>>>,
    subscribers: Mutex<Vec<SyncSender<Arc<Reading>>>>,
}

/// The loop's settings, under one lock so a wait sees them together.
struct Control {
    interval: Duration,
    frozen: bool,
    stopping: bool,
    /// Set by the first demand; the next tick's wait returns at once and
    /// clears it.
    woken: bool,
}

/// Clears the subscriber list when the loop ends, panic included, so a
/// `recv` on a stopped tracker returns `Disconnected` instead of blocking.
struct Exit<'a>(&'a Shared);

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                            Enums                             */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

/// Why a tick's wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    Elapsed,
    Woken,
    Stop,
}

/// What following the demand for processes did to the sampler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Follow {
    Unchanged,
    Changed,
    Stopped,
}

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                          Functions                           */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

impl Shared {
    pub(crate) fn new(interval: Duration) -> Self {
        Self {
            control: Mutex::new(Control {
                interval,
                frozen: false,
                stopping: false,
                woken: false,
            }),
            wake: Condvar::new(),
            demand: AtomicUsize::new(0),
            latest: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn interval(&self) -> Duration {
        locked(&self.control).interval
    }

    /// Clamped into range and applied at once: a tick's wait in progress
    /// moves its deadline.
    pub(crate) fn set_interval(&self, interval: Duration) {
        locked(&self.control).interval = interval.clamp(MIN_INTERVAL, MAX_INTERVAL);
        self.wake.notify_all();
    }

    pub(crate) fn is_frozen(&self) -> bool {
        locked(&self.control).frozen
    }

    pub(crate) fn set_frozen(&self, frozen: bool) {
        locked(&self.control).frozen = frozen;
    }

    /// Ends the current wait and, with it, the loop. Idempotent.
    pub(crate) fn stop(&self) {
        locked(&self.control).stopping = true;
        self.wake.notify_all();
    }

    /// Ends the current tick's wait early, once.
    pub(crate) fn wake(&self) {
        locked(&self.control).woken = true;
        self.wake.notify_all();
    }

    /// Sleeps until `tick` plus the interval, re-read whenever it changes;
    /// or less, when stopped or woken meanwhile.
    pub(crate) fn wait_tick(&self, tick: Instant) -> Wait {
        let mut control = locked(&self.control);
        loop {
            if control.stopping {
                return Wait::Stop;
            }
            if control.woken {
                control.woken = false;
                return Wait::Woken;
            }
            let Some(left) = remaining(tick, control.interval) else {
                return Wait::Elapsed;
            };
            control = wait_on(&self.wake, control, left);
        }
    }

    /// Sleeps `duration`, or until stopped: `false` then. A wake doesn't cut
    /// it short, so a baseline span is always a full one.
    pub(crate) fn pause(&self, duration: Duration) -> bool {
        let started = Instant::now();
        let mut control = locked(&self.control);
        loop {
            if control.stopping {
                return false;
            }
            let Some(left) = remaining(started, duration) else {
                return true;
            };
            control = wait_on(&self.wake, control, left);
        }
    }

    /// Live demand guards.
    pub(crate) fn demand(&self) -> usize {
        self.demand.load(Ordering::Acquire)
    }

    /// Adds a guard; returns how many there were before it.
    pub(crate) fn add_demand(&self) -> usize {
        self.demand.fetch_add(1, Ordering::AcqRel)
    }

    /// Removes a guard; only ever called from one's `Drop`.
    pub(crate) fn release_demand(&self) {
        self.demand.fetch_sub(1, Ordering::AcqRel);
    }

    pub(crate) fn latest(&self) -> Option<Arc<Reading>> {
        locked(&self.latest).clone()
    }

    /// A queue of readings for one consumer, bounded at `UPDATE_QUEUE`.
    pub(crate) fn subscribe(&self) -> Receiver<Arc<Reading>> {
        let (sender, receiver) = sync_channel(UPDATE_QUEUE);
        locked(&self.subscribers).push(sender);
        receiver
    }

    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        locked(&self.subscribers).len()
    }

    /// Makes `reading` the latest and offers it to every subscriber without
    /// waiting: a full queue misses it, a closed one is forgotten.
    pub(crate) fn publish(&self, reading: Arc<Reading>) {
        *locked(&self.latest) = Some(Arc::clone(&reading));
        locked(&self.subscribers).retain(|subscriber| {
            !matches!(
                subscriber.try_send(Arc::clone(&reading)),
                Err(TrySendError::Disconnected(_))
            )
        });
    }
}

impl Drop for Exit<'_> {
    fn drop(&mut self) {
        locked(&self.0.subscribers).clear();
    }
}

/// A lock's data, poisoned or not: a panic elsewhere must not take the
/// telemetry down with it.
pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `wait_timeout` with the same poisoning stance as [`locked`].
fn wait_on<'a>(
    condvar: &Condvar,
    guard: MutexGuard<'a, Control>,
    timeout: Duration,
) -> MutexGuard<'a, Control> {
    condvar
        .wait_timeout(guard, timeout)
        .unwrap_or_else(PoisonError::into_inner)
        .0
}

/// What is left of `duration` after `since`; `None` once it has run out,
/// and when the deadline can't be represented at all.
fn remaining(since: Instant, duration: Duration) -> Option<Duration> {
    since
        .checked_add(duration)?
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

/// The sampling loop, for as long as the tracker lives. Frozen, it samples
/// (so the rate baselines stay fresh) and publishes nothing.
pub(crate) fn run(mut sampler: Sampler, shared: Arc<Shared>, config: TrackerConfig) {
    let _exit = Exit(&shared);
    let mut history = History::new(config.history_len);
    let mut sequence = 0u64;
    let mut tick = Instant::now();
    while let Some(snapshot) = next_sample(&mut sampler, &shared, &mut tick) {
        if shared.is_frozen() {
            continue;
        }
        history.push(&snapshot);
        sequence = sequence.saturating_add(1);
        let reading = Reading::new(sequence, snapshot, history.clone(), config.busiest_len);
        shared.publish(Arc::new(reading));
    }
}

/// Waits out the tick, follows the demand for processes and takes the next
/// sample, moving `tick` to its start. `None` once stopped.
fn next_sample(sampler: &mut Sampler, shared: &Shared, tick: &mut Instant) -> Option<Snapshot> {
    loop {
        let ended = shared.wait_tick(*tick);
        if ended == Wait::Stop {
            return None;
        }
        match follow_demand(sampler, shared) {
            Follow::Stopped => return None,
            // The guard behind a wake went away before it was seen: nothing
            // changed, so the tick goes on where it was.
            Follow::Unchanged if ended == Wait::Woken => continue,
            Follow::Unchanged | Follow::Changed => {}
        }
        *tick = Instant::now();
        return Some(sampler.sample());
    }
}

/// Starts or stops listing processes to match the demand, giving process
/// rates a baseline span when starting.
fn follow_demand(sampler: &mut Sampler, shared: &Shared) -> Follow {
    let lists = shared.demand() > 0;
    if lists == sampler.lists_processes() {
        return Follow::Unchanged;
    }
    sampler.set_lists_processes(lists);
    if lists && !shared.pause(PROCESS_BASELINE) {
        return Follow::Stopped;
    }
    Follow::Changed
}
