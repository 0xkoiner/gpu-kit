//! Defaults and bounds of the pipeline: the plan's constants table.

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                      Tracker Constants                       */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

use std::time::Duration;

/* ´:°•.°+.*•´.*:˚.°*.˚•´.°:°•.°•.*•´.*:˚.°*.˚•´.°:°•.°+.*•´.*: */
/*                          Constants                           */
/* .•°:°.´+˚.*°.˚:*.´•*.+°.•°:´*.´•*.•°.•°:°.´:•˚°.*°.˚:*.´+°.• */

/// The wait between samples unless the config says otherwise: top's refresh.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(1);

/// The shortest wait: below it the /proc reads dominate the machine they
/// measure.
pub const MIN_INTERVAL: Duration = Duration::from_millis(100);

/// The longest wait, which keeps the deadline arithmetic of a wait in range.
pub const MAX_INTERVAL: Duration = Duration::from_secs(3600);

/// Samples a series keeps: the width of top's sparklines.
pub const HISTORY_LEN: usize = 48;

/// Processes a reading ranks: the length of top's popover list.
pub const BUSIEST_LEN: usize = 5;

/// How long process CPU rates accumulate after listing resumes, before the
/// sample that shows them.
pub const PROCESS_BASELINE: Duration = Duration::from_millis(250);

/// Readings a subscriber may lag before it misses some.
pub const UPDATE_QUEUE: usize = 8;

/// The sampling thread's name, as `ps -L` and a panic message show it.
pub const THREAD_NAME: &str = "telemetry-sampler";
