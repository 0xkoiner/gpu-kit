//! System metrics for the UI, read through each OS's own telemetry, with no
//! crate dependencies (the `serde` feature adds `Serialize` to the snapshot
//! types) and no root. On macOS that is raw FFI to IOKit, CoreFoundation,
//! libIOReport and libSystem, sampled the way mactop does. On Linux it is
//! /proc and /sys, with NVML (the NVIDIA driver's library) or the amdgpu
//! driver's sysfs files for the GPU.
//!
//! [`Sampler`] and the [`Snapshot`] it fills are the same on both; a metric
//! an OS or machine doesn't provide is `None`.
#![cfg(any(target_os = "macos", target_os = "linux"))]

mod cores;
mod rate;
mod signal;
mod snapshot;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub mod macos;

#[cfg(target_os = "linux")]
pub use linux::{Sampler, is_running, send_signal};
#[cfg(target_os = "macos")]
pub use macos::{ANE_FULL_LOAD_WATTS, Sampler, is_running, send_signal};
pub use signal::{Signal, SignalError};
pub use snapshot::*;
