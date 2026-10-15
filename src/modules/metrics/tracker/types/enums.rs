//! The metrics a history tracks, and their units.

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                        Tracker Enums                         */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

use crate::snapshot::{Memory, Snapshot};

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                            Enums                             */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

/// A metric the history keeps a series of: the closed set a chart can draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Metric {
    /// Mean busy share of all cores, 0–100.
    Cpu,
    /// Busy share of the GPU, 0–100.
    Gpu,
    /// Used share of physical memory, 0–100.
    Memory,
    /// Whole-system draw, or the sum of the metered blocks, in watts.
    Power,
    /// Bytes received per second over the machine's own interfaces.
    NetworkReceive,
    /// Bytes sent per second over the machine's own interfaces.
    NetworkSend,
    /// Bytes read per second from physical disks.
    DiskRead,
    /// Bytes written per second to physical disks.
    DiskWrite,
}

/// What a metric's values are measured in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Unit {
    /// A share, 0–100.
    Percent,
    /// Power draw.
    Watts,
    /// A transfer rate.
    BytesPerSecond,
}

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                          Functions                           */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

impl Metric {
    /// Every metric, in the order a dashboard lists them.
    pub const ALL: [Self; 8] = [
        Self::Cpu,
        Self::Gpu,
        Self::Memory,
        Self::Power,
        Self::NetworkReceive,
        Self::NetworkSend,
        Self::DiskRead,
        Self::DiskWrite,
    ];

    /// This metric's value in `snapshot`; `None` where the machine didn't
    /// provide it, so a chart draws a gap rather than an idle machine.
    pub fn read(self, snapshot: &Snapshot) -> Option<f32> {
        match self {
            Self::Cpu => snapshot.cpu.usage(),
            Self::Gpu => snapshot.gpu.map(|gpu| gpu.usage),
            Self::Memory => snapshot.memory.and_then(used_share),
            Self::Power => snapshot.power.total(),
            Self::NetworkReceive => snapshot.network.map(|rates| rates.read as f32),
            Self::NetworkSend => snapshot.network.map(|rates| rates.write as f32),
            Self::DiskRead => snapshot.disk.map(|rates| rates.read as f32),
            Self::DiskWrite => snapshot.disk.map(|rates| rates.write as f32),
        }
    }

    /// What this metric's values are measured in.
    pub fn unit(self) -> Unit {
        match self {
            Self::Cpu | Self::Gpu | Self::Memory => Unit::Percent,
            Self::Power => Unit::Watts,
            Self::NetworkReceive | Self::NetworkSend | Self::DiskRead | Self::DiskWrite => {
                Unit::BytesPerSecond
            }
        }
    }

    /// A stable snake_case name, for JSON keys and chart ids.
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Memory => "memory",
            Self::Power => "power",
            Self::NetworkReceive => "network_receive",
            Self::NetworkSend => "network_send",
            Self::DiskRead => "disk_read",
            Self::DiskWrite => "disk_write",
        }
    }
}

/// Used memory as a share of the total, 0–100. `None` for a total of 0,
/// which only an unread meminfo reports, so it can't pass for a full memory.
fn used_share(memory: Memory) -> Option<f32> {
    (memory.total > 0).then(|| (memory.used as f64 / memory.total as f64 * 100.0) as f32)
}
