use std::collections::HashMap;
use std::time::Instant;

use crate::modules::metrics::cf;
use crate::modules::metrics::rate::{ByteCounts, ByteRates};
use crate::modules::metrics::snapshot::Throughput;

/// Read/write byte counters of each physical block storage driver; rates come from deltas.
#[derive(Default)]
pub struct DiskCounters {
    rates: ByteRates<u64>,
}

impl DiskCounters {
    /// `None` on the first call, which only sets the baseline, and when no
    /// physical driver's counters can be read.
    pub fn sample(&mut self) -> Option<Throughput> {
        self.rates.sample(Instant::now(), read_drivers())
    }
}

/// Counters of each physical driver by registry ID, which survives disks
/// coming and going. `None` when the registry can't be searched, or when no
/// physical driver's counters can be read: every Mac boots from one.
fn read_drivers() -> Option<HashMap<u64, ByteCounts>> {
    let mut drivers = HashMap::new();
    let searched = cf::for_each_service("IOBlockStorageDriver", |driver| {
        if !is_virtual(driver)
            && let Some(id) = cf::registry_id(driver)
            && let Some(counts) = read_counts(driver)
        {
            drivers.insert(id, counts);
        }
    });
    (searched && !drivers.is_empty()).then_some(drivers)
}

/// A driver whose statistics can't be read is left out of this read rather
/// than counted as zero, so it can't later jump by its since-attach total.
fn read_counts(driver: cf::io_object_t) -> Option<ByteCounts> {
    let stats = cf::property(driver, "Statistics")?;
    // The kernel's counters are unsigned; CF hands them over as i64, and `as` restores the bits.
    let stat = |key| {
        cf::dict_get(&stats, key)
            .and_then(cf::number_i64)
            .map(|n| n as u64)
    };
    Some(ByteCounts {
        read: stat("Bytes (Read)")?,
        written: stat("Bytes (Write)")?,
    })
}

/// Disk images and RAM disks sit on a virtual interface. A disk image's I/O
/// also lands on the disk holding its file, so counting both would double it.
fn is_virtual(driver: cf::io_object_t) -> bool {
    cf::inherited_property(driver, "Protocol Characteristics").is_some_and(|protocol| {
        cf::dict_get(&protocol, "Physical Interconnect")
            .and_then(cf::string)
            .is_some_and(|interconnect| interconnect == "Virtual Interface")
    })
}
