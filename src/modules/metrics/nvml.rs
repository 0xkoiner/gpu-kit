//! NVIDIA's management library, loaded at run time: the driver installs it
//! as libnvidia-ml.so.1 beside nvidia-smi, so a machine without the driver
//! has none, and nothing else needs to link it. Signatures and constants
//! follow the CUDA toolkit's nvml.h.

use std::collections::HashMap;
use std::ffi::{CStr, c_uint, c_ulonglong, c_void};
use std::ptr;

use crate::gpu::DeviceSample;
use crate::sys;

type Device = *mut c_void;

/// `nvmlReturn_t` values.
const SUCCESS: c_uint = 0;
/// What `nvmlDeviceGetProcessUtilization` answers when no process has used
/// the GPU since the time asked about.
const NOT_FOUND: c_uint = 6;
const INSUFFICIENT_SIZE: c_uint = 7;
/// `nvmlClockType_t`: the SM (shader) clock.
const CLOCK_SM: c_uint = 1;
/// `nvmlTemperatureSensors_t`: the die sensor.
const TEMPERATURE_GPU: c_uint = 0;

/// `nvmlUtilization_t`.
#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

/// `nvmlProcessUtilizationSample_t`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ProcessSample {
    pid: c_uint,
    /// Microseconds since the epoch.
    timestamp: c_ulonglong,
    sm: c_uint,
    memory: c_uint,
    encoder: c_uint,
    decoder: c_uint,
}

const _: () = assert!(size_of::<ProcessSample>() == 32);

type Init = unsafe extern "C" fn() -> c_uint;
type Shutdown = unsafe extern "C" fn() -> c_uint;
type DeviceCount = unsafe extern "C" fn(*mut c_uint) -> c_uint;
type DeviceByIndex = unsafe extern "C" fn(c_uint, *mut Device) -> c_uint;
type GetUtilization = unsafe extern "C" fn(Device, *mut Utilization) -> c_uint;
type GetClock = unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_uint;
type GetUint = unsafe extern "C" fn(Device, *mut c_uint) -> c_uint;
type GetTemperature = unsafe extern "C" fn(Device, c_uint, *mut c_uint) -> c_uint;
type GetProcessUtilization =
    unsafe extern "C" fn(Device, *mut ProcessSample, *mut c_uint, c_ulonglong) -> c_uint;

/// A shared library, closed on drop.
struct Library(*mut c_void);

impl Library {
    fn open(name: &CStr) -> Option<Self> {
        let handle = unsafe { sys::dlopen(name.as_ptr(), sys::RTLD_NOW) };
        // Lazily: a `Library` of a null handle would `dlclose` it when dropped.
        (!handle.is_null()).then(|| Self(handle))
    }

    /// The function `name`, or `None` when the library lacks it.
    ///
    /// # Safety
    /// `F` must be the symbol's real signature.
    unsafe fn symbol<F: Copy>(&self, name: &CStr) -> Option<F> {
        const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
        let address = unsafe { sys::dlsym(self.0, name.as_ptr()) };
        // `dlsym` hands back the function's address; the caller vouches for its type.
        (!address.is_null())
            .then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&address) })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { sys::dlclose(self.0) };
    }
}

pub struct Nvml {
    /// Closed on drop, after `shutdown` has run.
    _library: Library,
    shutdown: Shutdown,
    utilization: GetUtilization,
    clock: GetClock,
    power: GetUint,
    temperature: GetTemperature,
    /// `nvmlDeviceGetNumGpuCores`, which drivers before 470 lack.
    cores: Option<GetUint>,
    /// `nvmlDeviceGetProcessUtilization`, which some GPUs don't support.
    process_utilization: Option<GetProcessUtilization>,
    devices: Vec<Device>,
    /// The newest process sample seen, so the next read asks for later ones only.
    last_seen: c_ulonglong,
}

// NVML's calls are thread-safe, and the sampler runs on one thread anyway.
unsafe impl Send for Nvml {}

impl Nvml {
    /// Loads the library and initialises it; `None` without the NVIDIA
    /// driver, or when it finds no GPU.
    pub fn open() -> Option<Self> {
        let library = Library::open(c"libnvidia-ml.so.1")?;
        // Each type is the signature nvml.h declares for the symbol.
        unsafe {
            let init: Init = library.symbol(c"nvmlInit_v2")?;
            let shutdown: Shutdown = library.symbol(c"nvmlShutdown")?;
            let device_count: DeviceCount = library.symbol(c"nvmlDeviceGetCount_v2")?;
            let device_by_index: DeviceByIndex =
                library.symbol(c"nvmlDeviceGetHandleByIndex_v2")?;
            let utilization: GetUtilization = library.symbol(c"nvmlDeviceGetUtilizationRates")?;
            let clock: GetClock = library.symbol(c"nvmlDeviceGetClockInfo")?;
            let power: GetUint = library.symbol(c"nvmlDeviceGetPowerUsage")?;
            let temperature: GetTemperature = library.symbol(c"nvmlDeviceGetTemperature")?;
            let cores: Option<GetUint> = library.symbol(c"nvmlDeviceGetNumGpuCores");
            let process_utilization: Option<GetProcessUtilization> =
                library.symbol(c"nvmlDeviceGetProcessUtilization");
            if init() != SUCCESS {
                return None;
            }
            let mut count = 0;
            let devices: Vec<Device> = if device_count(&mut count) == SUCCESS {
                (0..count)
                    .filter_map(|index| {
                        let mut device = ptr::null_mut();
                        (device_by_index(index, &mut device) == SUCCESS && !device.is_null())
                            .then_some(device)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if devices.is_empty() {
                shutdown();
                return None;
            }
            Some(Self {
                _library: library,
                shutdown,
                utilization,
                clock,
                power,
                temperature,
                cores,
                process_utilization,
                devices,
                last_seen: 0,
            })
        }
    }

    /// CUDA cores over every GPU; `None` where the driver can't count them.
    pub fn cores(&self) -> Option<u32> {
        let cores = self.cores?;
        self.devices
            .iter()
            .map(|&device| {
                let mut count = 0;
                (unsafe { cores(device, &mut count) } == SUCCESS).then_some(count)
            })
            .sum()
    }

    /// Each GPU's readings, and every process's share of GPU time since the
    /// last call. `None` shares unless every GPU reports process
    /// utilization; an empty map where no process used a GPU.
    pub fn sample(&mut self) -> (Vec<DeviceSample>, Option<HashMap<i32, f32>>) {
        let devices = self
            .devices
            .iter()
            .map(|&device| self.device_sample(device))
            .collect();
        let shares = self.sample_shares();
        (devices, shares)
    }

    /// Every process's share of GPU time since the last call: the most it
    /// took of any one GPU. `None` unless every GPU reports process
    /// utilization, as a process on one that doesn't would otherwise look
    /// idle.
    fn sample_shares(&mut self) -> Option<HashMap<i32, f32>> {
        let mut newest = self.last_seen;
        let answers: Vec<Option<HashMap<i32, f32>>> = self
            .devices
            .iter()
            .map(|&device| self.process_shares(device, self.last_seen, &mut newest))
            .collect();
        self.last_seen = newest;
        merge_all(answers)
    }

    fn device_sample(&self, device: Device) -> DeviceSample {
        let mut utilization = Utilization::default();
        let mut mhz = 0;
        let mut milliwatts = 0;
        let mut celsius = 0;
        // Each out-pointer is a local of the type the call writes.
        let usage = (unsafe { (self.utilization)(device, &mut utilization) } == SUCCESS)
            .then(|| utilization.gpu.min(100) as f32);
        let mhz = (unsafe { (self.clock)(device, CLOCK_SM, &mut mhz) } == SUCCESS).then_some(mhz);
        let watts = (unsafe { (self.power)(device, &mut milliwatts) } == SUCCESS)
            .then(|| milliwatts as f32 / 1000.0);
        let celsius = (unsafe { (self.temperature)(device, TEMPERATURE_GPU, &mut celsius) }
            == SUCCESS)
            .then_some(celsius as f32);
        DeviceSample {
            usage,
            mhz,
            watts,
            celsius,
        }
    }

    /// Each process's share of `device`'s time since `last_seen`, 0–100: its
    /// mean SM use over the driver's samples of it. `newest` takes the latest
    /// sample's timestamp. `None` where the device doesn't answer (a GPU that
    /// doesn't support the query, or a failed read), so an unknown share is
    /// never reported as none.
    fn process_shares(
        &self,
        device: Device,
        last_seen: c_ulonglong,
        newest: &mut c_ulonglong,
    ) -> Option<HashMap<i32, f32>> {
        let read = self.process_utilization?;
        let mut count: c_uint = 0;
        // Asked without a buffer, the call says how many samples there are,
        // or NOT_FOUND for none, which is an answer too.
        let probe = unsafe { read(device, ptr::null_mut(), &mut count, last_seen) };
        if probe == NOT_FOUND || (matches!(probe, SUCCESS | INSUFFICIENT_SIZE) && count == 0) {
            return Some(HashMap::new());
        }
        if !matches!(probe, SUCCESS | INSUFFICIENT_SIZE) {
            return None;
        }
        // Room for processes that start between the two calls.
        let mut samples = vec![ProcessSample::default(); (count as usize).min(4096) + 16];
        let mut filled = samples.len() as c_uint;
        if unsafe { read(device, samples.as_mut_ptr(), &mut filled, last_seen) } != SUCCESS {
            return None;
        }
        let filled = (filled as usize).min(samples.len());
        Some(mean_shares(samples.get(..filled).unwrap_or(&[]), newest))
    }
}

/// Every GPU's answer folded into one, a process keeping the most it took of
/// any GPU. `None` when a GPU didn't answer, or there is none: a process on a
/// GPU that can't report would otherwise look idle.
fn merge_all(
    answers: impl IntoIterator<Item = Option<HashMap<i32, f32>>>,
) -> Option<HashMap<i32, f32>> {
    let mut merged: HashMap<i32, f32> = HashMap::new();
    let mut any_gpu = false;
    for answer in answers {
        for (pid, share) in answer? {
            let most = merged.entry(pid).or_insert(0.0);
            *most = most.max(share);
        }
        any_gpu = true;
    }
    any_gpu.then_some(merged)
}

/// Each pid's mean SM share over its samples, 0–100; `newest` takes the
/// latest sample's timestamp.
fn mean_shares(samples: &[ProcessSample], newest: &mut c_ulonglong) -> HashMap<i32, f32> {
    let mut sums: HashMap<i32, (f32, u32)> = HashMap::new();
    for sample in samples {
        *newest = (*newest).max(sample.timestamp);
        if let Ok(pid) = i32::try_from(sample.pid) {
            let (sum, count) = sums.entry(pid).or_default();
            *sum += sample.sm.min(100) as f32;
            *count += 1;
        }
    }
    sums.into_iter()
        .map(|(pid, (sum, count))| (pid, sum / count as f32))
        .collect()
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // Before `_library` closes.
        unsafe { (self.shutdown)() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_no_gpu() {
        assert!(Library::open(c"libtop-metrics-no-such-library.so").is_none());
    }

    #[test]
    fn process_shares_need_every_gpu_to_answer() {
        let busy = HashMap::from([(7, 40.0), (9, 10.0)]);
        let busier = HashMap::from([(7, 60.0)]);
        assert_eq!(
            merge_all([Some(busy.clone()), Some(busier)]),
            Some(HashMap::from([(7, 60.0), (9, 10.0)])),
            "a process keeps the most it took of any GPU"
        );
        assert_eq!(
            merge_all([Some(busy), None]),
            None,
            "a GPU that can't answer would hide its processes as idle"
        );
        assert_eq!(
            merge_all([Some(HashMap::new()), Some(HashMap::new())]),
            Some(HashMap::new()),
            "idle GPUs are an answer"
        );
        assert_eq!(merge_all(Vec::new()), None, "no GPU is no answer");
    }

    #[test]
    fn symbols_resolve_from_a_loaded_library() {
        // Every process has libc; `getpid` has the signature of `Init`.
        let libc = Library::open(c"libc.so.6").expect("libc");
        let getpid: Init = unsafe { libc.symbol(c"getpid") }.expect("getpid");
        assert_eq!(unsafe { getpid() }, std::process::id());
        assert!(unsafe { libc.symbol::<Init>(c"no_such_symbol_in_libc") }.is_none());
    }
}
