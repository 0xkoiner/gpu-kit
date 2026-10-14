use std::collections::HashMap;
use std::ffi::CStr;
use std::time::Instant;

use crate::rate::{ByteCounts, ByteRates};
use crate::snapshot::Throughput;
use crate::sys::{self, ifmibdata};

/// 64-bit byte counters of each interface that carries its own traffic;
/// rates come from deltas.
#[derive(Default)]
pub struct NetworkCounters {
    rates: ByteRates<String>,
}

impl NetworkCounters {
    /// `read` is received bytes per second, `write` is sent. `None` on the
    /// first call, which only sets the baseline, and when the read fails.
    pub fn sample(&mut self) -> Option<Throughput> {
        self.rates.sample(Instant::now(), read_interfaces())
    }
}

/// Counters of each interface by name, so interfaces coming and going
/// (USB adapters, Thunderbolt docks) don't net against the others' traffic.
///
/// Read from `net.link.generic.ifalldata`, not `NET_RT_IFLIST2`: for a
/// non-Apple binary that one rounds the byte counters to 1 KiB and truncates
/// them to 32 bits, so they wrap every 4 GiB.
fn read_interfaces() -> Option<HashMap<String, ByteCounts>> {
    let mut mib = [
        sys::CTL_NET,
        sys::PF_LINK,
        sys::NETLINK_GENERIC,
        sys::IFMIB_IFALLDATA,
        0,
        sys::IFDATA_GENERAL,
    ];
    parse_interfaces(&sys::sysctl_table(&mut mib)?)
}

/// Parses a table of `struct ifmibdata`. `None` if it isn't whole records,
/// as then its layout isn't the one the offsets describe.
fn parse_interfaces(table: &[u8]) -> Option<HashMap<String, ByteCounts>> {
    if !table.len().is_multiple_of(ifmibdata::SIZE) {
        return None;
    }
    let interfaces = table
        .chunks_exact(ifmibdata::SIZE)
        .filter_map(|record| {
            let name = &record[ifmibdata::NAME..ifmibdata::NAME + ifmibdata::NAME_LEN];
            let name = CStr::from_bytes_until_nul(name).ok()?.to_str().ok()?;
            let counts = ByteCounts {
                read: sys::read_u64(record, ifmibdata::IBYTES),
                written: sys::read_u64(record, ifmibdata::OBYTES),
            };
            carries_own_traffic(name, record).then(|| (name.to_owned(), counts))
        })
        .collect();
    Some(interfaces)
}

/// Whether an interface's traffic is counted on it alone. Loopback never
/// leaves the Mac. Tunnels (utun, ipsec, ppp) carry traffic the link under
/// them counts again, encapsulated; bridges, VLANs and link aggregates count
/// what their member links already do; and a VM's `vmenet` end either stays
/// on the Mac or is forwarded through a physical link. An empty name is an
/// interface still attaching or already detaching.
fn carries_own_traffic(name: &str, record: &[u8]) -> bool {
    let flags = sys::read_u32(record, ifmibdata::FLAGS);
    let layered = matches!(
        record[ifmibdata::TYPE],
        sys::IFT_BRIDGE | sys::IFT_L2VLAN | sys::IFT_IEEE8023ADLAG
    );
    !name.is_empty()
        && flags & (sys::IFF_LOOPBACK | sys::IFF_POINTOPOINT) == 0
        && !layered
        && !name.starts_with("vmenet")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `IFF_UP`, `IFT_ETHER` and `IFT_OTHER`.
    const UP: u32 = 0x1;
    const ETHER: u8 = 0x6;
    const OTHER: u8 = 0x1;

    fn record(name: &str, flags: u32, kind: u8, read: u64, written: u64) -> Vec<u8> {
        let mut record = vec![0; ifmibdata::SIZE];
        record[ifmibdata::NAME..][..name.len()].copy_from_slice(name.as_bytes());
        record[ifmibdata::FLAGS..][..4].copy_from_slice(&flags.to_ne_bytes());
        record[ifmibdata::TYPE] = kind;
        record[ifmibdata::IBYTES..][..8].copy_from_slice(&read.to_ne_bytes());
        record[ifmibdata::OBYTES..][..8].copy_from_slice(&written.to_ne_bytes());
        record
    }

    #[test]
    fn counters_past_4_gib_read_whole() {
        // `NET_RT_IFLIST2` would hand these to us truncated to 32 bits.
        let table = record("en0", UP, ETHER, 5_000_000_000, 6_000_000_000);
        let en0 = parse_interfaces(&table).expect("whole records")["en0"];
        assert_eq!((en0.read, en0.written), (5_000_000_000, 6_000_000_000));
    }

    #[test]
    fn traffic_counted_on_another_interface_is_skipped() {
        let table = [
            record("en0", UP, ETHER, 1, 1),
            record("awdl0", UP, ETHER, 1, 1),
            record("lo0", UP | sys::IFF_LOOPBACK, sys::IFT_LOOP, 1, 1),
            record("utun3", UP | sys::IFF_POINTOPOINT, OTHER, 1, 1),
            record("bridge0", UP, sys::IFT_BRIDGE, 1, 1),
            record("vlan0", UP, sys::IFT_L2VLAN, 1, 1),
            record("bond0", UP, sys::IFT_IEEE8023ADLAG, 1, 1),
            record("vmenet0", UP, ETHER, 1, 1),
            // An interface mid-attach or detach comes back zeroed.
            vec![0; ifmibdata::SIZE],
        ]
        .concat();
        let interfaces = parse_interfaces(&table).expect("whole records");
        let mut names: Vec<_> = interfaces.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["awdl0", "en0"]);
    }

    #[test]
    fn a_table_of_partial_records_is_refused() {
        let mut table = record("en0", UP, ETHER, 1, 1);
        table.pop();
        assert!(parse_interfaces(&table).is_none());
    }
}
