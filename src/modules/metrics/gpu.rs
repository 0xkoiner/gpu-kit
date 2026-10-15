//! GPU telemetry from whichever driver the machine has: NVIDIA's through
//! NVML, AMD's through the amdgpu driver's sysfs files.

use std::collections::HashMap;

use crate::{amdgpu, nvml};

/// One GPU's readings; `None` where its driver doesn't report a value.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct DeviceSample {
    /// Busy share, 0–100.
    pub usage: Option<f32>,
    /// Graphics clock in MHz.
    pub mhz: Option<u32>,
    /// Board power in watts.
    pub watts: Option<f32>,
    pub celsius: Option<f32>,
}

/// One interval's readings over every GPU.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct GpuSample {
    /// Mean busy share across the GPUs, 0–100.
    pub usage: f32,
    /// Mean clock of the GPUs that report one, in MHz.
    pub mhz: Option<u32>,
    /// Board power over all GPUs; `None` unless every one reports it.
    pub watts: Option<f32>,
    /// The hottest GPU, in °C.
    pub celsius: Option<f32>,
    /// Each process's share of GPU time by pid, 0–100: the most it took of
    /// any one GPU. `Some` only when every GPU reports it, so a process on
    /// one that doesn't (amdgpu, some NVML devices) is never shown idle; an
    /// empty map where no process used a GPU.
    pub process_shares: Option<HashMap<i32, f32>>,
}

pub enum Gpus {
    Nvidia(nvml::Nvml),
    Amd(Vec<amdgpu::Card>),
}

impl Gpus {
    /// NVML first, which the NVIDIA driver installs beside nvidia-smi, then
    /// amdgpu. `None` with neither, or without a GPU.
    pub fn open() -> Option<Self> {
        if let Some(nvml) = nvml::Nvml::open() {
            return Some(Self::Nvidia(nvml));
        }
        let cards = amdgpu::cards();
        (!cards.is_empty()).then_some(Self::Amd(cards))
    }

    /// GPU cores over every GPU; `None` where the driver doesn't count them
    /// (amdgpu doesn't).
    pub fn cores(&self) -> Option<u32> {
        match self {
            Self::Nvidia(nvml) => nvml.cores(),
            Self::Amd(_) => None,
        }
    }

    /// `None` when no GPU reports how busy it is.
    pub fn sample(&mut self) -> Option<GpuSample> {
        let (devices, process_shares) = match self {
            Self::Nvidia(nvml) => nvml.sample(),
            Self::Amd(cards) => (cards.iter().map(amdgpu::Card::sample).collect(), None),
        };
        aggregate(&devices, process_shares)
    }
}

fn aggregate(
    devices: &[DeviceSample],
    process_shares: Option<HashMap<i32, f32>>,
) -> Option<GpuSample> {
    let usage: Vec<f32> = devices.iter().filter_map(|device| device.usage).collect();
    if usage.is_empty() {
        return None;
    }
    let clocks: Vec<u64> = devices
        .iter()
        .filter_map(|device| device.mhz)
        .map(u64::from)
        .collect();
    Some(GpuSample {
        usage: usage.iter().sum::<f32>() / usage.len() as f32,
        mhz: (!clocks.is_empty())
            .then(|| (clocks.iter().sum::<u64>() / clocks.len() as u64) as u32),
        watts: devices.iter().map(|device| device.watts).sum(),
        celsius: devices
            .iter()
            .filter_map(|device| device.celsius)
            .reduce(f32::max),
        process_shares,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpus_aggregate_to_one_reading() {
        let devices = [
            DeviceSample {
                usage: Some(50.0),
                mhz: Some(1500),
                watts: Some(100.0),
                celsius: Some(60.0),
            },
            DeviceSample {
                usage: Some(70.0),
                mhz: Some(1700),
                watts: None,
                celsius: Some(70.0),
            },
        ];
        let sample = aggregate(&devices, None).expect("busy GPUs");
        assert_eq!(sample.usage, 60.0);
        assert_eq!(sample.mhz, Some(1600));
        assert_eq!(sample.watts, None, "one GPU's power is unknown");
        assert_eq!(sample.celsius, Some(70.0));
        assert_eq!(
            sample.process_shares, None,
            "amdgpu reports no process shares"
        );
        // NVML with no process on the GPU: known, and nobody.
        let idle = aggregate(&devices, Some(HashMap::new())).expect("busy GPUs");
        assert_eq!(idle.process_shares, Some(HashMap::new()));
    }

    #[test]
    fn without_a_busy_reading_there_is_no_sample() {
        let devices = [DeviceSample {
            mhz: Some(1500),
            ..DeviceSample::default()
        }];
        assert!(aggregate(&devices, None).is_none());
        assert!(aggregate(&[], None).is_none());
    }
}
