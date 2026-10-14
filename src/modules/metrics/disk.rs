use std::collections::HashMap;
use std::time::Instant;

use crate::cf;
use crate::rate::{ByteCounts, ByteRates};
use crate::snapshot::Throughput;

/// Read/write byte counters of each physical block storage driver; rates come from deltas.
#[derive(Default)]
pub struct DiskCounters {
    rates: ByteRates<u64>,
    /// Whether each driver of the last read sits on a virtual interface, by
    /// registry ID. An entry's ancestry is fixed, so this saves searching it
    /// again on every read.
    virtual_drivers: HashMap<u64, bool>,
}

impl DiskCounters {
    /// `None` on the first call, which only sets the baseline, and when no
    /// physical driver's counters can be read.
    pub fn sample(&mut self) -> Option<Throughput> {
        self.rates
            .sample(Instant::now(), read_drivers(&mut self.virtual_drivers))
    }
}

/// Counters of each physical driver by registry ID, which survives disks
/// coming and going. `None` when the registry can't be searched, or when no
/// physical driver's counters can be read: every Mac boots from one.
///
/// `virtual_drivers` is the previous read's answers; it's replaced with this
/// read's, so drivers that went away drop out of it.
fn read_drivers(virtual_drivers: &mut HashMap<u64, bool>) -> Option<HashMap<u64, ByteCounts>> {
    let mut drivers = HashMap::new();
    let mut seen = HashMap::new();
    let searched = cf::for_each_service("IOBlockStorageDriver", |driver| {
        let Some(id) = cf::registry_id(driver) else {
            return;
        };
        let on_virtual = virtual_drivers
            .get(&id)
            .copied()
            .or_else(|| is_virtual(driver));
        if let Some(on_virtual) = on_virtual {
            seen.insert(id, on_virtual);
        }
        if on_virtual != Some(true)
            && let Some(counts) = read_counts(driver)
        {
            drivers.insert(id, counts);
        }
    });
    if searched {
        *virtual_drivers = seen;
    }
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
/// also lands on the disk holding its file, so counting both would double it;
/// a RAM disk's never reaches a disk, so it isn't disk traffic at all.
/// `None` when no ancestor states its interconnect: the driver is counted, and
/// asked again on the next read rather than remembered.
fn is_virtual(driver: cf::io_object_t) -> Option<bool> {
    let protocol = cf::inherited_property(driver, "Protocol Characteristics")?;
    Some(
        cf::dict_get(&protocol, "Physical Interconnect")
            .and_then(cf::string)
            .is_some_and(|interconnect| interconnect == "Virtual Interface"),
    )
}
