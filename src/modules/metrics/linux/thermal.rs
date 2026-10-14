//! Thermal pressure and the CPU's temperature, from the kernel's thermal
//! zones and hwmon devices.

use std::path::{Path, PathBuf};

use super::files;
use crate::snapshot::ThermalPressure;

const THERMAL: &str = "/sys/class/thermal";
const HWMON: &str = "/sys/class/hwmon";

/// The hwmon drivers that read a CPU's own sensors: Intel's coretemp, AMD's
/// k10temp (and the zenpower alternative), and ARM SoCs' thermal drivers.
const CPU_HWMON: [&str; 5] = [
    "coretemp",
    "k10temp",
    "zenpower",
    "cpu_thermal",
    "cpu-thermal",
];

/// How hard the kernel is holding temperatures down, from its thermal zones'
/// trip points: a zone at or past a `passive` trip is being throttled
/// (Moderate), past `hot` throttled hard (Heavy), past `critical` about to be
/// shut down (Trapping). The worst zone counts. `None` without thermal zones,
/// as in a VM; Sleeping is a macOS level.
pub fn pressure() -> Option<ThermalPressure> {
    pressure_in(Path::new(THERMAL))
}

fn pressure_in(thermal: &Path) -> Option<ThermalPressure> {
    let worst = zones(thermal).filter_map(|zone| zone_level(&zone)).max()?;
    Some(match worst {
        0 => ThermalPressure::Nominal,
        1 => ThermalPressure::Moderate,
        2 => ThermalPressure::Heavy,
        _ => ThermalPressure::Trapping,
    })
}

fn zones(thermal: &Path) -> impl Iterator<Item = PathBuf> {
    files::entries(thermal)
        .into_iter()
        .filter(|zone| files::name(zone).is_some_and(|name| name.starts_with("thermal_zone")))
}

/// 0 to 3 for the worst kind of trip point a zone's temperature has reached;
/// `None` when the zone can't be read.
fn zone_level(zone: &Path) -> Option<u8> {
    let temp: i64 = files::read_number(zone.join("temp"))?;
    let level = files::entries(zone)
        .iter()
        .filter_map(|trip| {
            let index = files::name(trip)?
                .strip_prefix("trip_point_")?
                .strip_suffix("_type")?;
            let threshold: i64 = files::read_number(zone.join(format!("trip_point_{index}_temp")))?;
            let kind = files::read(trip)?;
            // A trip at or below 0 m°C is disabled, not a threshold.
            (threshold > 0 && temp >= threshold).then(|| trip_level(&kind))
        })
        .max()
        .unwrap_or(0);
    Some(level)
}

fn trip_level(kind: &str) -> u8 {
    match kind {
        "passive" => 1,
        "hot" => 2,
        "critical" => 3,
        _ => 0,
    }
}

/// Mean of the CPU's temperature sensors in °C, from the hwmon device its
/// driver registers, else from the thermal zones named for the CPU or SoC.
/// A reading outside 0–150 °C is a sensor with nothing behind it. `None`
/// without any, as in a VM.
pub fn cpu_temperature() -> Option<f32> {
    cpu_temperature_in(Path::new(HWMON), Path::new(THERMAL))
}

fn cpu_temperature_in(hwmon: &Path, thermal: &Path) -> Option<f32> {
    let mut readings: Vec<f32> = files::entries(hwmon)
        .iter()
        .filter(|device| {
            files::read(device.join("name")).is_some_and(|name| CPU_HWMON.contains(&name.as_str()))
        })
        .flat_map(|device| temp_inputs(device))
        .collect();
    if readings.is_empty() {
        readings = zones(thermal)
            .filter(|zone| files::read(zone.join("type")).is_some_and(|kind| is_cpu_zone(&kind)))
            .filter_map(|zone| files::read_number::<i64>(zone.join("temp")))
            .map(celsius)
            .collect();
    }
    mean_plausible(&readings)
}

fn is_cpu_zone(kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    kind.contains("cpu") || kind.contains("soc") || kind == "x86_pkg_temp"
}

/// Every `temp*_input` of an hwmon device, in °C.
fn temp_inputs(device: &Path) -> Vec<f32> {
    files::entries(device)
        .iter()
        .filter(|file| {
            files::name(file)
                .is_some_and(|name| name.starts_with("temp") && name.ends_with("_input"))
        })
        .filter_map(files::read_number::<i64>)
        .map(celsius)
        .collect()
}

/// sysfs reports temperatures in thousandths of a degree.
fn celsius(millidegrees: i64) -> f32 {
    millidegrees as f32 / 1000.0
}

fn mean_plausible(readings: &[f32]) -> Option<f32> {
    let plausible: Vec<f32> = readings
        .iter()
        .copied()
        .filter(|celsius| *celsius > 0.0 && *celsius <= 150.0)
        .collect();
    (!plausible.is_empty()).then(|| plausible.iter().sum::<f32>() / plausible.len() as f32)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A fake sysfs class directory of devices, each a directory of `(file, contents)`.
    fn fake_class(name: &str, devices: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("top-metrics-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (device, files) in devices {
            let dir = root.join(device);
            fs::create_dir_all(&dir).expect("fixture dir");
            for (file, contents) in *files {
                fs::write(dir.join(file), format!("{contents}\n")).expect("fixture file");
            }
        }
        root
    }

    #[test]
    fn pressure_is_the_worst_trip_point_reached() {
        let thermal = fake_class(
            "thermal",
            &[
                (
                    "thermal_zone0",
                    &[
                        ("type", "x86_pkg_temp"),
                        ("temp", "85000"),
                        ("trip_point_0_type", "passive"),
                        ("trip_point_0_temp", "80000"),
                        ("trip_point_1_type", "critical"),
                        ("trip_point_1_temp", "100000"),
                    ],
                ),
                (
                    "thermal_zone1",
                    &[
                        ("type", "acpitz"),
                        ("temp", "40000"),
                        ("trip_point_0_type", "critical"),
                        ("trip_point_0_temp", "0"),
                    ],
                ),
                ("cooling_device0", &[("type", "Processor")]),
            ],
        );
        assert_eq!(pressure_in(&thermal), Some(ThermalPressure::Moderate));
        fs::write(thermal.join("thermal_zone0/temp"), "100000\n").expect("fixture");
        assert_eq!(pressure_in(&thermal), Some(ThermalPressure::Trapping));
        fs::write(thermal.join("thermal_zone0/temp"), "50000\n").expect("fixture");
        assert_eq!(pressure_in(&thermal), Some(ThermalPressure::Nominal));
        // The CPU zone is the one named for it.
        assert_eq!(
            cpu_temperature_in(&thermal.join("none"), &thermal),
            Some(50.0)
        );
        let _ = fs::remove_dir_all(thermal);
    }

    #[test]
    fn without_thermal_zones_there_is_no_pressure() {
        let thermal = fake_class("no-zones", &[("cooling_device0", &[("type", "Fan")])]);
        assert_eq!(pressure_in(&thermal), None);
        assert_eq!(cpu_temperature_in(&thermal.join("none"), &thermal), None);
        let _ = fs::remove_dir_all(thermal);
    }

    #[test]
    fn cpu_temperature_is_the_mean_of_its_hwmon_sensors() {
        let hwmon = fake_class(
            "hwmon",
            &[
                ("hwmon0", &[("name", "nvme"), ("temp1_input", "70000")]),
                (
                    "hwmon1",
                    &[
                        ("name", "coretemp"),
                        ("temp1_label", "Package id 0"),
                        ("temp1_input", "45000"),
                        ("temp2_input", "47000"),
                        // A core with no sensor behind it.
                        ("temp3_input", "0"),
                    ],
                ),
            ],
        );
        assert_eq!(cpu_temperature_in(&hwmon, &hwmon.join("none")), Some(46.0));
        let _ = fs::remove_dir_all(hwmon);
    }
}
