//! CPU package, DRAM and platform power from RAPL's energy counters, through
//! the powercap sysfs tree.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::files;

const POWERCAP: &str = "/sys/class/powercap";

/// Mean watts over an interval, by RAPL domain. `None` where the chip has no
/// such domain or a counter of it couldn't be read.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Energy {
    /// The CPU packages, cores and uncore together.
    pub cpu: Option<f32>,
    pub dram: Option<f32>,
    /// The whole platform (`psys`), which few chips report.
    pub system: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Domain {
    Package,
    Dram,
    Platform,
}

struct Counter {
    domain: Domain,
    energy: PathBuf,
    /// The counter wraps past this many µJ; unknown, a wrap can't be read.
    range: Option<u64>,
}

pub struct Rapl {
    counters: Vec<Counter>,
    previous: Option<(Instant, Vec<Option<u64>>)>,
}

impl Rapl {
    /// `None` without a readable counter: VMs rarely expose RAPL, and since
    /// the PLATYPUS attack (2020) kernels open `energy_uj` to root alone.
    pub fn open() -> Option<Self> {
        Self::open_in(Path::new(POWERCAP))
    }

    fn open_in(powercap: &Path) -> Option<Self> {
        let counters: Vec<Counter> = discover(powercap)
            .into_iter()
            .filter(|counter| files::read_number::<u64>(&counter.energy).is_some())
            .collect();
        (!counters.is_empty()).then_some(Self {
            counters,
            previous: None,
        })
    }

    /// Mean watts since the last call; `None` the first time, which only sets
    /// the baseline.
    pub fn sample(&mut self) -> Option<Energy> {
        self.sample_at(Instant::now())
    }

    fn sample_at(&mut self, now: Instant) -> Option<Energy> {
        let readings: Vec<Option<u64>> = self
            .counters
            .iter()
            .map(|counter| files::read_number(&counter.energy))
            .collect();
        let (at, before) = self.previous.replace((now, readings.clone()))?;
        let secs = now.duration_since(at).as_secs_f64().max(1e-3);
        let watts: Vec<Option<f64>> = self
            .counters
            .iter()
            .zip(before)
            .zip(&readings)
            .map(|((counter, before), &after)| {
                Some(microjoules_moved(before, after, counter.range)? as f64 / 1e6 / secs)
            })
            .collect();
        let total = |domain| {
            let mut sum = 0.0;
            let mut any = false;
            for (counter, watts) in self.counters.iter().zip(&watts) {
                if counter.domain == domain {
                    sum += (*watts)?;
                    any = true;
                }
            }
            any.then_some(sum as f32)
        };
        Some(Energy {
            cpu: total(Domain::Package),
            dram: total(Domain::Dram),
            system: total(Domain::Platform),
        })
    }
}

/// How far a counter advanced, allowing one wrap when its range is known.
fn microjoules_moved(before: Option<u64>, after: Option<u64>, range: Option<u64>) -> Option<u64> {
    let (before, after) = (before?, after?);
    if after >= before {
        Some(after - before)
    } else {
        range?
            .checked_sub(before)?
            .checked_add(after)?
            .checked_add(1)
    }
}

/// The RAPL zones under powercap: `intel-rapl:N` is a package (or on some
/// laptops the `psys` platform), `intel-rapl:N:M` one of its sub-domains, of
/// which `dram` is wanted and core and uncore are already inside the package.
/// The `intel-rapl-mmio` zones repeat the same counters through another
/// register and would double them. AMD's chips appear under the same names.
fn discover(powercap: &Path) -> Vec<Counter> {
    files::entries(powercap)
        .into_iter()
        .filter(|zone| files::name(zone).is_some_and(|name| name.starts_with("intel-rapl:")))
        .filter_map(|zone| {
            let name = files::read(zone.join("name"))?;
            let domain = match name.as_str() {
                "dram" => Domain::Dram,
                "psys" => Domain::Platform,
                package if package.starts_with("package-") => Domain::Package,
                _ => return None,
            };
            Some(Counter {
                domain,
                energy: zone.join("energy_uj"),
                range: files::read_number(zone.join("max_energy_range_uj")),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::*;

    /// A fake powercap tree named for the test, as the tests run in parallel.
    fn fake_powercap(name: &str, zones: &[(&str, &str, u64)]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("top-metrics-rapl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (zone, name, energy) in zones {
            let dir = root.join(zone);
            fs::create_dir_all(&dir).expect("fixture dir");
            fs::write(dir.join("name"), format!("{name}\n")).expect("fixture");
            fs::write(dir.join("energy_uj"), format!("{energy}\n")).expect("fixture");
            fs::write(dir.join("max_energy_range_uj"), "262143328850\n").expect("fixture");
        }
        root
    }

    fn set_energy(root: &Path, zone: &str, energy: u64) {
        fs::write(root.join(zone).join("energy_uj"), format!("{energy}\n")).expect("fixture");
    }

    #[test]
    fn watts_are_the_energy_moved_per_domain_over_the_interval() {
        let root = fake_powercap(
            "domains",
            &[
                ("intel-rapl:0", "package-0", 1_000_000),
                ("intel-rapl:0:0", "core", 500_000),
                ("intel-rapl:0:1", "dram", 200_000),
                ("intel-rapl:1", "package-1", 262_143_328_850),
                // The same package through MMIO, which must not count twice.
                ("intel-rapl-mmio:0", "package-0", 1_000_000),
            ],
        );
        let mut rapl = Rapl::open_in(&root).expect("readable counters");
        let start = Instant::now();
        assert_eq!(
            rapl.sample_at(start),
            None,
            "the first read is the baseline"
        );
        set_energy(&root, "intel-rapl:0", 21_000_000);
        set_energy(&root, "intel-rapl:0:1", 1_200_000);
        // Package 1 wrapped: 999_999 µJ past the end of its range.
        set_energy(&root, "intel-rapl:1", 999_999);
        let energy = rapl
            .sample_at(start + Duration::from_secs(2))
            .expect("a second read");
        // (20 J + 1 J) / 2 s; DRAM 1 J / 2 s; no psys.
        assert_eq!(
            energy,
            Energy {
                cpu: Some(10.5),
                dram: Some(0.5),
                system: None
            }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unreadable_counter_blanks_its_domain_alone() {
        let root = fake_powercap(
            "unreadable",
            &[
                ("intel-rapl:0", "package-0", 1_000_000),
                ("intel-rapl:0:1", "dram", 200_000),
            ],
        );
        let mut rapl = Rapl::open_in(&root).expect("readable counters");
        let start = Instant::now();
        rapl.sample_at(start);
        set_energy(&root, "intel-rapl:0", 3_000_000);
        fs::remove_file(root.join("intel-rapl:0:1/energy_uj")).expect("fixture");
        let energy = rapl
            .sample_at(start + Duration::from_secs(1))
            .expect("a second read");
        assert_eq!(
            energy,
            Energy {
                cpu: Some(2.0),
                dram: None,
                system: None
            }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_backwards_counter_of_unknown_range_has_no_reading() {
        assert_eq!(microjoules_moved(Some(5), Some(3), None), None);
        assert_eq!(microjoules_moved(Some(5), Some(3), Some(9)), Some(8));
        assert_eq!(microjoules_moved(None, Some(3), Some(9)), None);
    }

    #[test]
    fn without_rapl_there_is_nothing_to_open() {
        let root = fake_powercap("empty", &[]);
        assert!(Rapl::open_in(&root).is_none());
        assert!(Rapl::open_in(&root.join("missing")).is_none());
        let _ = fs::remove_dir_all(root);
    }
}
