//! Interface byte counters from /proc/net/dev; rates come from deltas.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::files;
use crate::rate::{ByteCounts, ByteRates};
use crate::snapshot::Throughput;

const NET: &str = "/sys/class/net";

/// `IFF_LOOPBACK` and `IFF_POINTOPOINT` from <net/if.h>, as `flags` prints them.
const IFF_LOOPBACK: u64 = 0x8;
const IFF_POINTOPOINT: u64 = 0x10;

/// Byte counters of each interface that carries its own traffic; rates come
/// from deltas.
#[derive(Default)]
pub struct NetworkCounters {
    rates: ByteRates<String>,
}

impl NetworkCounters {
    /// `read` is received bytes per second, `write` is sent. `None` on the
    /// first call, which only sets the baseline, when the read fails, and
    /// when no interface carries its own traffic.
    pub fn sample(&mut self) -> Option<Throughput> {
        self.rates.sample(Instant::now(), read_interfaces())
    }
}

/// Counters of each interface by name, so interfaces coming and going (USB
/// adapters, containers' veth ends) don't net against the others' traffic.
/// `None` when /proc/net/dev can't be read, and when no interface carries
/// its own traffic, so a rate of zero is a quiet network, not a missing one.
fn read_interfaces() -> Option<HashMap<String, ByteCounts>> {
    let counts = parse_counts(&files::read("/proc/net/dev")?);
    let interfaces = own_traffic(counts, |name| describe(Path::new(NET), name));
    (!interfaces.is_empty()).then_some(interfaces)
}

/// `(name, counts)` of every interface in /proc/net/dev, whose two header
/// lines are followed by `name: rx_bytes rx_packets ... tx_bytes ...`, the
/// transmit bytes being the ninth number.
fn parse_counts(dev: &str) -> Vec<(String, ByteCounts)> {
    dev.lines()
        .filter_map(|line| {
            let (name, fields) = line.split_once(':')?;
            let fields: Vec<&str> = fields.split_whitespace().collect();
            let counts = ByteCounts {
                read: fields.first()?.parse().ok()?,
                written: fields.get(8)?.parse().ok()?,
            };
            Some((name.trim().to_owned(), counts))
        })
        .collect()
}

/// What sysfs says of an interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Link {
    loopback: bool,
    point_to_point: bool,
    /// Backed by a device (a NIC, or a VM's virtual one), not by other links.
    physical: bool,
}

fn describe(net: &Path, name: &str) -> Link {
    let dir = net.join(name);
    let flags = files::read(dir.join("flags"))
        .and_then(|flags| u64::from_str_radix(flags.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    Link {
        loopback: flags & IFF_LOOPBACK != 0,
        point_to_point: flags & IFF_POINTOPOINT != 0,
        physical: dir.join("device").exists(),
    }
}

/// The interfaces whose traffic is counted on them alone. Loopback never
/// leaves the machine. Tunnels (point-to-point: tun, ppp, WireGuard) carry
/// traffic the link under them counts again, encapsulated. Of the rest, only
/// those backed by a device count: a bridge, bond, VLAN, macvlan or veth end
/// moves what a physical link already counts. A machine without any such
/// device, such as a container whose network is one veth end, counts every
/// remaining interface instead: those are its network.
fn own_traffic(
    counts: Vec<(String, ByteCounts)>,
    describe: impl Fn(&str) -> Link,
) -> HashMap<String, ByteCounts> {
    let links: Vec<(String, ByteCounts, Link)> = counts
        .into_iter()
        .map(|(name, counts)| {
            let link = describe(&name);
            (name, counts, link)
        })
        .filter(|(_, _, link)| !link.loopback && !link.point_to_point)
        .collect();
    let any_physical = links.iter().any(|(_, _, link)| link.physical);
    links
        .into_iter()
        .filter(|(_, _, link)| link.physical || !any_physical)
        .map(|(name, counts, _)| (name, counts))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEV: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo:  351447     649    0    0    0     0          0         0   351447     649    0    0    0     0       0          0
  eth0: 5767584   35889    0    6    0     0          0         0 35816579   36167    0    0    0     0       0          0
";

    #[test]
    fn received_and_sent_bytes_are_read_per_interface() {
        let counts = parse_counts(DEV);
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[1].0, "eth0");
        assert_eq!(
            (counts[1].1.read, counts[1].1.written),
            (5_767_584, 35_816_579)
        );
    }

    fn links<'a>(table: &'a [(&'a str, Link)]) -> impl Fn(&str) -> Link + 'a {
        move |name| {
            table
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, link)| *link)
                .expect("a known interface")
        }
    }

    fn counts(names: &[&str]) -> Vec<(String, ByteCounts)> {
        names
            .iter()
            .map(|name| (name.to_string(), ByteCounts::default()))
            .collect()
    }

    fn sorted(interfaces: HashMap<String, ByteCounts>) -> Vec<String> {
        let mut names: Vec<String> = interfaces.into_keys().collect();
        names.sort();
        names
    }

    const PHYSICAL: Link = Link {
        loopback: false,
        point_to_point: false,
        physical: true,
    };
    const VIRTUAL: Link = Link {
        physical: false,
        ..PHYSICAL
    };

    #[test]
    fn traffic_counted_on_another_interface_is_skipped() {
        let table = [
            (
                "lo",
                Link {
                    loopback: true,
                    ..VIRTUAL
                },
            ),
            ("ens5", PHYSICAL),
            ("wlan0", PHYSICAL),
            (
                "wg0",
                Link {
                    point_to_point: true,
                    ..VIRTUAL
                },
            ),
            ("docker0", VIRTUAL),
            ("veth1a2b", VIRTUAL),
            ("bond0", VIRTUAL),
        ];
        let names: Vec<&str> = table.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            sorted(own_traffic(counts(&names), links(&table))),
            ["ens5", "wlan0"]
        );
    }

    #[test]
    fn a_container_counts_its_virtual_interfaces() {
        let table = [
            (
                "lo",
                Link {
                    loopback: true,
                    ..VIRTUAL
                },
            ),
            ("eth0", VIRTUAL),
            (
                "tun0",
                Link {
                    point_to_point: true,
                    ..VIRTUAL
                },
            ),
        ];
        assert_eq!(
            sorted(own_traffic(counts(&["lo", "eth0", "tun0"]), links(&table))),
            ["eth0"]
        );
    }

    #[test]
    fn this_machine_has_an_interface_with_its_own_traffic() {
        let interfaces = read_interfaces().expect("/proc/net/dev");
        assert!(!interfaces.is_empty());
        assert!(!interfaces.contains_key("lo"));
    }
}
