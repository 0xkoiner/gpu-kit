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
    /// Set by the first demand; the next wait returns at once and clears it.
    woken: bool,
}

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                            Enums                             */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

/// Why a wait ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wait {
    Elapsed,
    Woken,
    Stop,
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

    /// Clamped into range; the loop reads it at its next top.
    pub(crate) fn set_interval(&self, interval: Duration) {
        locked(&self.control).interval = interval.clamp(MIN_INTERVAL, MAX_INTERVAL);
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

    /// Ends the current wait early, once.
    pub(crate) fn wake(&self) {
        locked(&self.control).woken = true;
        self.wake.notify_all();
    }

    /// Sleeps `duration`, or less when stopped or woken meanwhile.
    pub(crate) fn wait(&self, duration: Duration) -> Wait {
        let Some(deadline) = Instant::now().checked_add(duration) else {
            return Wait::Elapsed;
        };
        let mut control = locked(&self.control);
        loop {
            if control.stopping {
                return Wait::Stop;
            }
            if control.woken {
                control.woken = false;
                return Wait::Woken;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Wait::Elapsed;
            }
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

/// The sampling loop, for as long as the tracker lives. Frozen, it samples
/// (so the rate baselines stay fresh) and publishes nothing.
pub(crate) fn run(mut sampler: Sampler, shared: Arc<Shared>, config: TrackerConfig) {
    let mut history = History::new(config.history_len);
    let mut sequence = 0u64;
    loop {
        if shared.wait(shared.interval()) == Wait::Stop || !follow_demand(&mut sampler, &shared) {
            break;
        }
        let snapshot = sampler.sample();
        if shared.is_frozen() {
            continue;
        }
        history.push(&snapshot);
        sequence = sequence.saturating_add(1);
        let reading = Reading::new(sequence, snapshot, history.clone(), config.busiest_len);
        shared.publish(Arc::new(reading));
    }
}

/// Starts or stops listing processes to match the demand, giving process
/// rates a baseline span when starting. `false` when stopped meanwhile.
fn follow_demand(sampler: &mut Sampler, shared: &Shared) -> bool {
    let lists = shared.demand() > 0;
    if lists == sampler.lists_processes() {
        return true;
    }
    sampler.set_lists_processes(lists);
    !(lists && shared.wait(PROCESS_BASELINE) == Wait::Stop)
}
