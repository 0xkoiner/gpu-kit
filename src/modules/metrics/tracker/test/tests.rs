//! Tickets 01–05: the pure window and ranking first, then the live loop on
//! this machine.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

use super::types::constants::{
    MAX_HISTORY_LEN, MAX_INTERVAL, MIN_INTERVAL, PROCESS_BASELINE, UPDATE_QUEUE,
};
use super::{History, Metric, Reading, Series, Tracker, TrackerConfig, Unit};
use crate::snapshot::{
    CoreKind, CoreUsage, Cpu, Gpu, Memory, Power, Process, Snapshot, Throughput,
};

/// Plenty for a `MIN_INTERVAL` tracker on a loaded CI machine.
const PATIENCE: Duration = Duration::from_secs(5);

fn series_of(capacity: usize, values: &[Option<f32>]) -> Series {
    let mut series = Series::new(capacity);
    for &value in values {
        series.push(value);
    }
    series
}

fn core(usage: f32) -> CoreUsage {
    CoreUsage {
        kind: CoreKind::Performance,
        usage,
    }
}

/// A machine with every metric readable: CPU 50 %, GPU 70 %, memory 25 %,
/// 12.5 W, and distinct rates.
fn busy_snapshot() -> Snapshot {
    Snapshot {
        cpu: Cpu {
            cores: vec![core(40.0), core(60.0)],
            ..Cpu::default()
        },
        gpu: Some(Gpu {
            usage: 70.0,
            mhz: None,
        }),
        memory: Some(Memory {
            used: 1,
            total: 4,
            swap: None,
        }),
        power: Power {
            system: Some(12.5),
            ..Power::default()
        },
        network: Some(Throughput {
            read: 1000.0,
            write: 2000.0,
        }),
        disk: Some(Throughput {
            read: 3.0,
            write: 4.0,
        }),
        ..Snapshot::default()
    }
}

#[test]
fn a_series_keeps_the_newest_values_and_the_peak_outlives_the_window() {
    let mut series = Series::new(48);
    series.push(Some(99.0));
    for value in 0..48 {
        series.push(Some(value as f32));
    }
    assert_eq!(series.len(), 48);
    assert_eq!(series.values().next(), Some(Some(0.0)));
    assert_eq!(series.latest(), Some(47.0));
    assert_eq!(series.max(), Some(47.0), "the window's own maximum");
    assert_eq!(
        series.peak(),
        Some(99.0),
        "the dropped value is still the peak"
    );
    assert_eq!(series.capacity(), 48);
}

#[test]
fn mean_and_max_skip_gaps_and_an_all_gap_window_has_neither() {
    let series = series_of(4, &[Some(10.0), None, Some(30.0), Some(f32::NAN)]);
    assert_eq!(series.mean(), Some(20.0));
    assert_eq!(series.max(), Some(30.0));
    assert_eq!(series.peak(), Some(30.0));
    let gaps = series_of(4, &[None, None]);
    assert_eq!(
        (gaps.mean(), gaps.max(), gaps.peak(), gaps.latest()),
        (None, None, None, None)
    );
    assert_eq!(
        Series::new(0).capacity(),
        1,
        "a window holds at least one value"
    );
}

#[test]
fn a_nan_first_value_leaves_no_peak() {
    let mut series = series_of(2, &[Some(f32::NAN)]);
    assert_eq!(series.peak(), None);
    series.push(Some(3.0));
    assert_eq!(series.peak(), Some(3.0));
    assert_eq!(
        Series::new(usize::MAX).capacity(),
        MAX_HISTORY_LEN,
        "a window never asks for more memory than a day of samples"
    );
}

#[test]
fn latest_is_none_right_after_a_gap() {
    let series = series_of(4, &[Some(1.0), None]);
    assert_eq!(series.latest(), None);
    assert_eq!(series.len(), 2);
    assert!(!series.is_empty());
}

#[test]
fn metrics_read_their_value_from_a_snapshot() {
    let snapshot = busy_snapshot();
    let values: Vec<Option<f32>> = Metric::ALL
        .iter()
        .map(|metric| metric.read(&snapshot))
        .collect();
    assert_eq!(
        values,
        [
            Some(50.0),
            Some(70.0),
            Some(25.0),
            Some(12.5),
            Some(1000.0),
            Some(2000.0),
            Some(3.0),
            Some(4.0)
        ]
    );
    assert_eq!(Metric::Memory.unit(), Unit::Percent);
    assert_eq!(Metric::Power.unit(), Unit::Watts);
    assert_eq!(Metric::DiskWrite.unit(), Unit::BytesPerSecond);
    assert_eq!(Metric::NetworkReceive.label(), "network_receive");
}

#[test]
fn metrics_are_none_where_the_snapshot_has_none() {
    let empty = Snapshot::default();
    assert!(
        Metric::ALL
            .iter()
            .all(|metric| metric.read(&empty).is_none())
    );
    let no_total = Snapshot {
        memory: Some(Memory {
            used: 5,
            total: 0,
            swap: None,
        }),
        ..Snapshot::default()
    };
    assert_eq!(
        Metric::Memory.read(&no_total),
        None,
        "a zero total is an unread meminfo, not a full memory"
    );
}

#[test]
fn a_history_grows_every_series_by_one() {
    let mut history = History::new(3);
    assert!(history.is_empty());
    history.push(&busy_snapshot());
    history.push(&Snapshot::default());
    assert_eq!(history.len(), 2);
    assert_eq!(history.capacity(), 3);
    for metric in Metric::ALL {
        let series = history.series(metric);
        assert_eq!(series.len(), 2, "{metric:?}");
        assert_eq!(
            series.latest(),
            None,
            "{metric:?}: the empty snapshot is a gap"
        );
        assert!(
            series.peak().is_some(),
            "{metric:?}: the busy snapshot set the peak"
        );
    }
}

#[test]
fn a_config_is_clamped_into_range() {
    let short = TrackerConfig {
        interval: Duration::from_millis(1),
        history_len: 0,
        busiest_len: 0,
    }
    .normalized();
    assert_eq!(
        (short.interval, short.history_len, short.busiest_len),
        (MIN_INTERVAL, 1, 0)
    );
    let long = TrackerConfig {
        interval: Duration::from_secs(7200),
        ..TrackerConfig::default()
    }
    .normalized();
    assert_eq!(long.interval, MAX_INTERVAL);
    assert_eq!(TrackerConfig::default().history_len, 48);
}

/// A snapshot listing one process per share, pids in order.
fn processes(shares: impl IntoIterator<Item = f32>) -> Snapshot {
    let processes = shares
        .into_iter()
        .enumerate()
        .map(|(ix, cpu)| Process {
            pid: ix as i32,
            cpu,
            ..Process::default()
        })
        .collect();
    Snapshot {
        processes: Some(processes),
        ..Snapshot::default()
    }
}

/// The CPU shares of a reading's busiest processes, in rank order.
fn ranked(snapshot: Snapshot, count: usize) -> Vec<f32> {
    Reading::new(1, snapshot, History::new(1), count)
        .busiest()
        .map(|process| process.cpu)
        .collect()
}

#[test]
fn readings_rank_the_busiest_processes() {
    let shares = (0..20).map(|ix| (ix * 7 % 20) as f32);
    assert_eq!(ranked(processes(shares), 5), [19.0, 18.0, 17.0, 16.0, 15.0]);
}

#[test]
fn busiest_is_empty_without_processes_or_without_a_count() {
    assert!(ranked(Snapshot::default(), 5).is_empty());
    assert!(ranked(processes([1.0, 2.0]), 0).is_empty());
}

#[test]
fn fewer_processes_than_the_count_are_all_ranked() {
    assert_eq!(ranked(processes([1.0, 3.0, 2.0]), 5), [3.0, 2.0, 1.0]);
}

#[test]
fn a_nan_share_ranks_as_idle() {
    let shares = ranked(processes([f32::NAN, 5.0, 0.0]), 3);
    assert_eq!(shares.len(), 3);
    assert_eq!(shares.first().copied(), Some(5.0));
    assert!(
        shares
            .iter()
            .skip(1)
            .all(|share| share.is_nan() || *share == 0.0)
    );
}

#[test]
fn reading_accessors_return_what_was_given() {
    let before = SystemTime::now();
    let mut history = History::new(2);
    history.push(&busy_snapshot());
    let reading = Reading::new(7, busy_snapshot(), history, 5);
    assert_eq!(reading.sequence(), 7);
    assert!(reading.taken_at() >= before);
    assert_eq!(reading.history().len(), 1);
    assert_eq!(reading.snapshot().cpu.usage(), Some(50.0));
}

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                      Live: this machine                      */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

fn tracker(interval: Duration) -> Tracker {
    Tracker::start(TrackerConfig {
        interval,
        ..TrackerConfig::default()
    })
    .expect("the OS gives us a thread")
}

fn next(updates: &Receiver<Arc<Reading>>) -> Arc<Reading> {
    updates
        .recv_timeout(PATIENCE)
        .expect("a reading within PATIENCE")
}

/// Readings until one satisfies `done`, within `PATIENCE` in total.
fn next_where(
    updates: &Receiver<Arc<Reading>>,
    mut done: impl FnMut(&Reading) -> bool,
) -> Arc<Reading> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let reading = updates
            .recv_timeout(left)
            .expect("a matching reading within PATIENCE");
        if done(&reading) {
            return reading;
        }
    }
}

#[test]
fn a_tracker_publishes_its_first_reading_after_one_interval() {
    let started = Instant::now();
    let interval = Duration::from_millis(500);
    let tracker = tracker(interval);
    let updates = tracker.subscribe();
    if started.elapsed() < interval / 2 {
        assert!(
            tracker.latest().is_none(),
            "nothing before the first interval"
        );
    }
    let first = next(&updates);
    assert_eq!(first.sequence(), 1);
    assert_eq!(first.history().len(), 1);
    assert!(first.snapshot().uptime_secs > 0);
    assert!(first.history().series(Metric::Cpu).latest().is_some());
    assert!(
        tracker
            .latest()
            .is_some_and(|latest| latest.sequence() >= 1)
    );
    assert!(!tracker.chip().tiers.is_empty());
    assert!(tracker.is_alive());
}

#[test]
fn a_frozen_tracker_publishes_nothing_and_resumes_in_sequence() {
    let tracker = tracker(MIN_INTERVAL);
    let updates = tracker.subscribe();
    let _ = next(&updates);
    tracker.set_frozen(true);
    assert!(tracker.is_frozen());
    // A sample already under way may still land; after one interval none can.
    std::thread::sleep(MIN_INTERVAL);
    let held = tracker.latest().expect("the first reading");
    while updates.try_recv().is_ok() {}
    std::thread::sleep(MIN_INTERVAL * 3);
    assert_eq!(
        tracker.latest().map(|latest| latest.sequence()),
        Some(held.sequence()),
        "frozen, nothing is published"
    );
    assert!(updates.try_recv().is_err());
    tracker.set_frozen(false);
    let resumed = next(&updates);
    assert_eq!(resumed.sequence(), held.sequence() + 1);
    assert_eq!(resumed.history().len(), held.history().len() + 1);
}

#[test]
fn interval_is_clamped() {
    let tracker = tracker(MIN_INTERVAL);
    tracker.set_interval(Duration::ZERO);
    assert_eq!(tracker.interval(), MIN_INTERVAL);
    tracker.set_interval(Duration::from_secs(86_400));
    assert_eq!(tracker.interval(), MAX_INTERVAL);
}

#[test]
fn stop_returns_promptly_even_at_the_longest_interval() {
    let tracker = tracker(MAX_INTERVAL);
    assert!(tracker.is_alive());
    let begun = Instant::now();
    tracker.stop();
    assert!(
        begun.elapsed() < Duration::from_secs(2),
        "{:?}",
        begun.elapsed()
    );
    assert!(!tracker.is_alive());
    assert!(
        tracker.latest().is_none(),
        "nothing is published in an hour-long wait"
    );
}

#[test]
fn processes_are_listed_only_while_demanded() {
    let tracker = tracker(MIN_INTERVAL);
    let updates = tracker.subscribe();
    assert!(next(&updates).snapshot().processes.is_none());
    assert!(!tracker.lists_processes());
    let demand = tracker.demand_processes();
    assert!(tracker.lists_processes());
    let listed = next_where(&updates, |reading| reading.snapshot().processes.is_some());
    assert!(
        listed
            .snapshot()
            .processes
            .as_ref()
            .is_some_and(|processes| !processes.is_empty())
    );
    assert!(listed.busiest().next().is_some());
    drop(demand);
    assert!(!tracker.lists_processes());
    next_where(&updates, |reading| reading.snapshot().processes.is_none());
}

#[test]
fn two_guards_release_independently() {
    let tracker = tracker(MAX_INTERVAL);
    let first = tracker.demand_processes();
    let second = tracker.demand_processes();
    drop(first);
    assert!(tracker.lists_processes());
    drop(second);
    assert!(!tracker.lists_processes());
}

#[test]
fn the_first_guard_wakes_a_long_wait() {
    let tracker = tracker(MAX_INTERVAL);
    let updates = tracker.subscribe();
    let _demand = tracker.demand_processes();
    let reading = next(&updates);
    assert!(reading.snapshot().processes.is_some());
}

#[test]
fn subscribers_receive_readings_in_order() {
    let tracker = tracker(MIN_INTERVAL);
    let updates = tracker.subscribe();
    let sequences: Vec<u64> = (0..3).map(|_| next(&updates).sequence()).collect();
    assert_eq!(sequences, [1, 2, 3]);
    assert!(
        tracker
            .latest()
            .is_some_and(|latest| latest.sequence() >= 3)
    );
}

#[test]
fn a_stalled_subscriber_does_not_block_the_others() {
    let tracker = tracker(MIN_INTERVAL);
    let _stalled = tracker.subscribe();
    let reader = tracker.subscribe();
    for expected in 1..=(UPDATE_QUEUE as u64 + 4) {
        assert_eq!(next(&reader).sequence(), expected);
    }
    assert!(tracker.is_alive());
    assert_eq!(
        tracker.subscriber_count(),
        2,
        "a full queue is kept, not dropped"
    );
}

#[test]
fn a_dropped_receiver_is_forgotten_at_the_next_publish() {
    let tracker = tracker(MIN_INTERVAL);
    let dropped = tracker.subscribe();
    let kept = tracker.subscribe();
    assert_eq!(tracker.subscriber_count(), 2);
    drop(dropped);
    // A reading numbered past what is latest now was published after the drop.
    let held = tracker.latest().map_or(0, |latest| latest.sequence());
    next_where(&kept, |reading| reading.sequence() > held);
    assert_eq!(tracker.subscriber_count(), 1);
}

#[test]
fn subscribers_disconnect_once_the_tracker_has_stopped() {
    let tracker = tracker(MAX_INTERVAL);
    let updates = tracker.subscribe();
    tracker.stop();
    assert!(
        matches!(
            updates.recv_timeout(PATIENCE),
            Err(RecvTimeoutError::Disconnected)
        ),
        "a receiver on a stopped tracker must not block forever"
    );
}

#[test]
fn dropping_a_tracker_lets_its_thread_exit() {
    let tracker = tracker(MAX_INTERVAL);
    let shared = Arc::downgrade(&tracker.shared);
    drop(tracker);
    let deadline = Instant::now() + PATIENCE;
    while shared.strong_count() > 0 {
        assert!(
            Instant::now() < deadline,
            "the thread still holds the shared block"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_shorter_interval_applies_to_the_wait_in_progress() {
    let tracker = tracker(MAX_INTERVAL);
    let updates = tracker.subscribe();
    let begun = Instant::now();
    tracker.set_interval(MIN_INTERVAL);
    let first = next(&updates);
    assert_eq!(first.sequence(), 1);
    assert!(begun.elapsed() < PATIENCE);
}

#[test]
fn the_process_baseline_is_not_cut_short_by_a_second_guard() {
    let tracker = tracker(MAX_INTERVAL);
    let updates = tracker.subscribe();
    let begun = Instant::now();
    let first = tracker.demand_processes();
    std::thread::sleep(Duration::from_millis(60));
    drop(first);
    let _second = tracker.demand_processes();
    let reading = next(&updates);
    assert!(
        begun.elapsed() >= PROCESS_BASELINE,
        "sampled {:?} after the baseline was recorded",
        begun.elapsed()
    );
    assert!(reading.snapshot().processes.is_some());
}

#[test]
fn a_guard_gone_before_it_is_seen_takes_no_sample() {
    let tracker = tracker(MAX_INTERVAL);
    let updates = tracker.subscribe();
    let _ = tracker.demand_processes();
    assert!(
        matches!(
            updates.recv_timeout(Duration::from_secs(1)),
            Err(RecvTimeoutError::Timeout)
        ),
        "a wake that changes nothing is not a tick"
    );
    assert!(tracker.is_alive());
}
