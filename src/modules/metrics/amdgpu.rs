//! AMD GPUs through the amdgpu driver's sysfs files under /sys/class/drm.

use std::path::{Path, PathBuf};

use crate::files;
use crate::gpu::DeviceSample;

const DRM: &str = "/sys/class/drm";

/// AMD's PCI vendor ID, as `vendor` prints it.
const AMD: &str = "0x1002";

/// One GPU the amdgpu driver runs.
pub struct Card {
    busy: PathBuf,
    /// Its hwmon directory, which holds the clock, power and temperature.
    hwmon: Option<PathBuf>,
}

/// Every AMD GPU: the `cardN` entries (not their `cardN-<connector>`
/// outputs) whose device is AMD's and has amdgpu's `gpu_busy_percent`.
pub fn cards() -> Vec<Card> {
    cards_in(Path::new(DRM))
}

fn cards_in(drm: &Path) -> Vec<Card> {
    files::entries(drm)
        .into_iter()
        .filter(|card| {
            files::name(card).is_some_and(|name| {
                name.strip_prefix("card").is_some_and(|index| {
                    !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit())
                })
            })
        })
        .filter_map(|card| {
            let device = card.join("device");
            if files::read(device.join("vendor"))? != AMD {
                return None;
            }
            let busy = device.join("gpu_busy_percent");
            busy.exists().then(|| Card {
                busy,
                hwmon: files::entries(device.join("hwmon")).into_iter().next(),
            })
        })
        .collect()
}

impl Card {
    pub fn sample(&self) -> DeviceSample {
        let hwmon = |file: &str| {
            self.hwmon
                .as_ref()
                .and_then(|hwmon| files::read_number::<i64>(hwmon.join(file)))
        };
        DeviceSample {
            usage: files::read_number::<u32>(&self.busy).map(|percent| percent.min(100) as f32),
            // `freq1_input` is the graphics clock in Hz.
            mhz: hwmon("freq1_input")
                .map(|hz| (hz / 1_000_000).clamp(0, i64::from(u32::MAX)) as u32),
            // Microwatts; kernels before 6.7 average it as `power1_average`.
            watts: hwmon("power1_input")
                .or_else(|| hwmon("power1_average"))
                .map(|microwatts| microwatts as f32 / 1e6),
            // Thousandths of a degree.
            celsius: hwmon("temp1_input").map(|millidegrees| millidegrees as f32 / 1000.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(path: PathBuf, contents: &str) {
        fs::create_dir_all(path.parent().expect("a file path")).expect("fixture dir");
        fs::write(path, format!("{contents}\n")).expect("fixture file");
    }

    #[test]
    fn amd_cards_are_found_and_read() {
        let drm = std::env::temp_dir().join(format!("top-metrics-drm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&drm);
        write(drm.join("card0/device/vendor"), "0x1002");
        write(drm.join("card0/device/gpu_busy_percent"), "37");
        write(
            drm.join("card0/device/hwmon/hwmon3/freq1_input"),
            "1750000000",
        );
        write(
            drm.join("card0/device/hwmon/hwmon3/power1_average"),
            "123456789",
        );
        write(drm.join("card0/device/hwmon/hwmon3/temp1_input"), "61000");
        // A connector of the card, and an NVIDIA card, are not amdgpu's.
        write(drm.join("card0-DP-1/status"), "connected");
        write(drm.join("card1/device/vendor"), "0x10de");
        fs::create_dir_all(drm.join("renderD128")).expect("fixture dir");

        let cards = cards_in(&drm);
        assert_eq!(cards.len(), 1);
        let sample = cards[0].sample();
        assert_eq!(sample.usage, Some(37.0));
        assert_eq!(sample.mhz, Some(1750));
        assert!(
            sample
                .watts
                .is_some_and(|watts| (watts - 123.457).abs() < 0.001)
        );
        assert_eq!(sample.celsius, Some(61.0));
        let _ = fs::remove_dir_all(drm);
    }
}
