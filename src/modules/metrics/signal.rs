//! Signalling a process the sampler listed. Each OS's `send_signal` and
//! `is_running` live beside its process table.

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
