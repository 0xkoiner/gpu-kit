//! The machine: its chip, uptime and battery, from /proc and /sys.

use std::path::{Path, PathBuf};

use super::{cpu, files, memory};
use crate::snapshot::{Battery, ChipInfo};

const POWER_SUPPLY: &str = "/sys/class/power_supply";

/// The chip's name, core tiers and memory, with the GPU core count the GPU
/// driver gave. The name falls back to "Unknown CPU" and memory to 0 when
/// /proc doesn't say.
pub fn chip_info(gpu_cores: u32) -> ChipInfo {
    ChipInfo {
        name: chip_name(),
        tiers: cpu::core_tiers(),
        gpu_cores,
        memory_bytes: memory::total_bytes().unwrap_or(0),
    }
}

fn chip_name() -> String {
    files::read("/proc/cpuinfo")
        .and_then(|cpuinfo| parse_chip_name(&cpuinfo, device_tree_model))
        .unwrap_or_else(|| "Unknown CPU".to_owned())
}

/// A board's name from its device tree, as Raspberry Pis have; the file ends
/// in a NUL.
fn device_tree_model() -> Option<String> {
    files::read("/proc/device-tree/model")
        .map(|model| model.trim_end_matches('\0').to_owned())
        .filter(|model| !model.is_empty())
}

/// x86's `model name`, with its runs of spaces squeezed. ARM's cpuinfo names
/// no chip, only the implementer and part numbers, so a board's device tree
/// model comes next, then the core those numbers name: AWS's Graviton 2, 3 and
/// 4 are Neoverse N1, V1 and V2 cores.
fn parse_chip_name(
    cpuinfo: &str,
    device_tree_model: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if let Some(name) = files::field(cpuinfo, "model name") {
        return Some(name.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    if let Some(model) = device_tree_model() {
        return Some(model);
    }
    let implementer = files::field(cpuinfo, "CPU implementer")?;
    let part = files::field(cpuinfo, "CPU part")?;
    Some(
        arm_core(implementer, part)
            .map_or_else(|| format!("ARM {implementer} part {part}"), str::to_owned),
    )
}

/// Arm Ltd's (implementer 0x41) part numbers, from the kernel's <asm/cputype.h>.
fn arm_core(implementer: &str, part: &str) -> Option<&'static str> {
    if implementer != "0x41" {
        return None;
    }
    Some(
        match u32::from_str_radix(part.trim_start_matches("0x"), 16).ok()? {
            0xd03 => "Cortex-A53",
            0xd07 => "Cortex-A57",
            0xd08 => "Cortex-A72",
            0xd09 => "Cortex-A73",
            0xd0a => "Cortex-A75",
            0xd0b => "Cortex-A76",
            0xd0d => "Cortex-A77",
            0xd41 => "Cortex-A78",
            0xd0c => "Neoverse-N1",
            0xd49 => "Neoverse-N2",
            0xd40 => "Neoverse-V1",
            0xd4f => "Neoverse-V2",
            _ => return None,
        },
    )
}

/// Seconds since boot, sleep included, as /proc/uptime counts them. 0 when it
/// can't be read.
pub fn uptime_secs() -> u64 {
    files::read("/proc/uptime")
        .and_then(|uptime| uptime.split_whitespace().next()?.parse::<f64>().ok())
        .map_or(0, |secs| secs as u64)
}

/// Charge of the machine's own batteries and whether it runs on mains power.
/// `None` without a battery. A wireless mouse's battery has `scope` Device
/// and isn't the machine's.
pub fn battery() -> Option<Battery> {
    battery_in(Path::new(POWER_SUPPLY))
}

fn battery_in(power_supply: &Path) -> Option<Battery> {
    let supplies = files::entries(power_supply);
    let batteries: Vec<&PathBuf> = supplies
        .iter()
        .filter(|supply| is_system_battery(supply))
        .collect();
    let charges: Vec<u32> = batteries
        .iter()
        .filter_map(|battery| charge(battery))
        .collect();
    if charges.is_empty() {
        return None;
    }
    let percent = charges.iter().sum::<u32>() / charges.len() as u32;
    // Mains power is a supply of type Mains, or USB for a USB-C charger, that
    // is online. A machine that lists none says through its battery's status.
    let mains: Vec<&PathBuf> = supplies
        .iter()
        .filter(|supply| {
            matches!(
                files::read(supply.join("type")).as_deref(),
                Some("Mains" | "USB")
            )
        })
        .collect();
    let on_ac = if mains.is_empty() {
        batteries.iter().any(|battery| {
            files::read(battery.join("status")).is_some_and(|status| status != "Discharging")
        })
    } else {
        mains
            .iter()
            .any(|supply| files::read_number::<u8>(supply.join("online")) == Some(1))
    };
    Some(Battery { percent, on_ac })
}

fn is_system_battery(supply: &Path) -> bool {
    files::read(supply.join("type")).as_deref() == Some("Battery")
        && files::read(supply.join("scope")).is_none_or(|scope| scope != "Device")
}

/// A battery's charge, 0–100: its `capacity`, or the share of full it holds,
/// in energy or in charge.
fn charge(battery: &Path) -> Option<u32> {
    if let Some(capacity) = files::read_number::<u32>(battery.join("capacity")) {
        return Some(capacity.min(100));
    }
    [("energy_now", "energy_full"), ("charge_now", "charge_full")]
        .into_iter()
        .find_map(|(now, full)| {
            let now: u64 = files::read_number(battery.join(now))?;
            let full: u64 = files::read_number(battery.join(full))?;
            (full > 0).then(|| (now.saturating_mul(100) / full).min(100) as u32)
        })
}

/// Whole-system draw while on battery, as the discharging batteries report
/// it: `power_now`, or current times voltage. On mains Linux has nothing to
/// meter the machine with, so `None` then, as without a battery.
pub fn battery_power() -> Option<f32> {
    battery_power_in(Path::new(POWER_SUPPLY))
}

fn battery_power_in(power_supply: &Path) -> Option<f32> {
    let watts: f64 = files::entries(power_supply)
        .iter()
        .filter(|supply| {
            is_system_battery(supply)
                && files::read(supply.join("status")).as_deref() == Some("Discharging")
        })
        .filter_map(|battery| {
            // Microwatts, or microamps times microvolts.
            if let Some(microwatts) = files::read_number::<u64>(battery.join("power_now")) {
                return Some(microwatts as f64 / 1e6);
            }
            let microamps = files::read_number::<i64>(battery.join("current_now"))?;
            let microvolts: u64 = files::read_number(battery.join("voltage_now"))?;
            Some(microamps.unsigned_abs() as f64 * microvolts as f64 / 1e12)
        })
        .sum();
    (watts > 0.0 && watts.is_finite()).then_some(watts as f32)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn x86_chips_go_by_their_model_name() {
        let cpuinfo =
            "processor\t: 0\nmodel name\t: Intel(R) Xeon(R)  Platinum 8259CL CPU @ 2.50GHz\n";
        assert_eq!(
            parse_chip_name(cpuinfo, || Some("ignored".into())).as_deref(),
            Some("Intel(R) Xeon(R) Platinum 8259CL CPU @ 2.50GHz")
        );
    }

    #[test]
    fn arm_chips_go_by_their_board_then_their_core() {
        let graviton2 =
            "processor\t: 0\nBogoMIPS\t: 243.75\nCPU implementer\t: 0x41\nCPU part\t: 0xd0c\n";
        assert_eq!(
            parse_chip_name(graviton2, || None).as_deref(),
            Some("Neoverse-N1")
        );
        assert_eq!(
            parse_chip_name(graviton2, || Some("Raspberry Pi 4 Model B".into())).as_deref(),
            Some("Raspberry Pi 4 Model B")
        );
        let apple = "CPU implementer\t: 0x61\nCPU part\t: 0x023\n";
        assert_eq!(
            parse_chip_name(apple, || None).as_deref(),
            Some("ARM 0x61 part 0x023")
        );
        assert!(parse_chip_name("flags\t: fpu\n", || None).is_none());
    }

    #[test]
    fn this_machine_has_a_chip_and_an_uptime() {
        let chip = chip_info(0);
        assert_ne!(chip.name, "Unknown CPU");
        assert!(chip.memory_bytes > 0);
        assert!(uptime_secs() > 0);
    }

    /// A fake /sys/class/power_supply with the given supplies, each a
    /// directory of `(file, contents)`.
    fn fake_supplies(name: &str, supplies: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("top-metrics-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (supply, files) in supplies {
            let dir = root.join(supply);
            fs::create_dir_all(&dir).expect("fixture dir");
            for (file, contents) in *files {
                fs::write(dir.join(file), format!("{contents}\n")).expect("fixture file");
            }
        }
        root
    }

    #[test]
    fn battery_charge_and_mains_power_are_read() {
        let root = fake_supplies(
            "battery",
            &[
                ("AC", &[("type", "Mains"), ("online", "1")]),
                (
                    "BAT0",
                    &[
                        ("type", "Battery"),
                        ("capacity", "73"),
                        ("status", "Charging"),
                    ],
                ),
                // A wireless mouse's battery is not the machine's.
                (
                    "hidpp_battery_0",
                    &[("type", "Battery"), ("scope", "Device"), ("capacity", "5")],
                ),
            ],
        );
        let battery = battery_in(&root).expect("a battery");
        assert_eq!((battery.percent, battery.on_ac), (73, true));
        assert_eq!(battery_power_in(&root), None, "charging, not discharging");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_discharging_battery_meters_the_machine() {
        let root = fake_supplies(
            "discharging",
            &[(
                "BAT0",
                &[
                    ("type", "Battery"),
                    ("energy_now", "30000000"),
                    ("energy_full", "60000000"),
                    ("status", "Discharging"),
                    ("current_now", "-1500000"),
                    ("voltage_now", "12000000"),
                ],
            )],
        );
        let battery = battery_in(&root).expect("a battery");
        assert_eq!((battery.percent, battery.on_ac), (50, false));
        assert_eq!(battery_power_in(&root), Some(18.0));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_machine_without_a_battery_has_none() {
        let root = fake_supplies(
            "mains-only",
            &[("AC", &[("type", "Mains"), ("online", "1")])],
        );
        assert!(battery_in(&root).is_none());
        assert_eq!(battery_power_in(&root), None);
        let _ = fs::remove_dir_all(root);
    }
}
