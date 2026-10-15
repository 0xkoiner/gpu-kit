//! The bounded window each metric keeps, and its aggregates.

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                           History                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

use std::collections::VecDeque;

use super::types::constants::MAX_HISTORY_LEN;
use super::types::enums::Metric;
use super::types::structs::{History, Series};
use crate::snapshot::Snapshot;

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                            Series                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

impl Series {
    /// An empty series that keeps the last `capacity` values, clamped to
    /// `1..=MAX_HISTORY_LEN`.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.clamp(1, MAX_HISTORY_LEN);
        Self {
            values: VecDeque::with_capacity(capacity),
            capacity,
            peak: None,
        }
    }

    /// Appends a value, dropping the oldest once the window is full. A present
    /// value raises the peak; a gap or a NaN leaves it.
    pub fn push(&mut self, value: Option<f32>) {
        if self.values.len() >= self.capacity {
            self.values.pop_front();
        }
        self.values.push_back(value);
        self.peak = match (self.peak, value) {
            (Some(peak), Some(value)) => Some(peak.max(value)),
            (None, Some(value)) if !value.is_nan() => Some(value),
            (peak, _) => peak,
        };
    }

    /// The values, oldest first.
    pub fn values(&self) -> impl Iterator<Item = Option<f32>> + '_ {
        self.values.iter().copied()
    }

    /// The last pushed value; `None` for a gap or an empty series.
    pub fn latest(&self) -> Option<f32> {
        self.values.back().copied().flatten()
    }

    /// The largest present value in the window.
    pub fn max(&self) -> Option<f32> {
        self.present().reduce(f32::max)
    }

    /// The mean of the window's present values; `None` without any.
    pub fn mean(&self) -> Option<f32> {
        let (sum, count) = self.present().fold((0.0f64, 0u32), |(sum, count), value| {
            (sum + f64::from(value), count.saturating_add(1))
        });
        (count > 0).then(|| (sum / f64::from(count)) as f32)
    }

    /// The largest value ever pushed, which outlives the window.
    pub fn peak(&self) -> Option<f32> {
        self.peak
    }

    /// Values held, gaps included.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Values the window holds at most.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether nothing has been pushed yet.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// The window's values that are numbers: no gaps, no NaN.
    fn present(&self) -> impl Iterator<Item = f32> + '_ {
        self.values
            .iter()
            .flatten()
            .copied()
            .filter(|value| !value.is_nan())
    }
}

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                           History                            */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

impl History {
    /// Empty series of `capacity` for every metric.
    pub fn new(capacity: usize) -> Self {
        let series = || Series::new(capacity);
        Self {
            cpu: series(),
            gpu: series(),
            memory: series(),
            power: series(),
            network_receive: series(),
            network_send: series(),
            disk_read: series(),
            disk_write: series(),
        }
    }

    /// Appends each metric's value in `snapshot` to its series, so every
    /// series stays the same length.
    pub fn push(&mut self, snapshot: &Snapshot) {
        for metric in Metric::ALL {
            self.series_mut(metric).push(metric.read(snapshot));
        }
    }

    /// The series of one metric.
    pub fn series(&self, metric: Metric) -> &Series {
        match metric {
            Metric::Cpu => &self.cpu,
            Metric::Gpu => &self.gpu,
            Metric::Memory => &self.memory,
            Metric::Power => &self.power,
            Metric::NetworkReceive => &self.network_receive,
            Metric::NetworkSend => &self.network_send,
            Metric::DiskRead => &self.disk_read,
            Metric::DiskWrite => &self.disk_write,
        }
    }

    /// Samples held, the same for every series.
    pub fn len(&self) -> usize {
        self.cpu.len()
    }

    /// Samples every series holds at most.
    pub fn capacity(&self) -> usize {
        self.cpu.capacity()
    }

    /// Whether no snapshot has been pushed yet.
    pub fn is_empty(&self) -> bool {
        self.cpu.is_empty()
    }

    fn series_mut(&mut self, metric: Metric) -> &mut Series {
        match metric {
            Metric::Cpu => &mut self.cpu,
            Metric::Gpu => &mut self.gpu,
            Metric::Memory => &mut self.memory,
            Metric::Power => &mut self.power,
            Metric::NetworkReceive => &mut self.network_receive,
            Metric::NetworkSend => &mut self.network_send,
            Metric::DiskRead => &mut self.disk_read,
            Metric::DiskWrite => &mut self.disk_write,
        }
    }
}
