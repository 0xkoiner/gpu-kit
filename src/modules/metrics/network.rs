use std::collections::HashMap;
use std::time::Instant;

use crate::modules::metrics::rate::{ByteCounts, ByteRates};
use crate::modules::metrics::snapshot::Throughput;
use crate::modules::metrics::sys;

/// 64-bit byte counters of each non-loopback interface; rates come from deltas.
#[derive(Default)]
pub struct NetworkCounters {
    rates: ByteRates<u16>,
}

impl NetworkCounters {
    /// `read` is received bytes per second, `write` is sent. `None` on the
    /// first call, which only sets the baseline, and when the read fails.
    pub fn sample(&mut self) -> Option<Throughput> {
        self.rates.sample(Instant::now(), read_interfaces())
    }
}

// `struct if_msghdr2` from <net/if.h>: msglen u16 @0, type u8 @3, flags i32 @8,
// index u16 @12, then `struct if_data64` @32 with ibytes u64 @+64 and obytes u64 @+72.
const MSG_LEN: usize = 0;
const MSG_TYPE: usize = 3;
const MSG_FLAGS: usize = 8;
const MSG_INDEX: usize = 12;
const IBYTES: usize = 32 + 64;
const OBYTES: usize = 32 + 72;

/// Counters of each interface by index, so interfaces coming and going
/// (VPN tunnels, USB adapters) don't net against the others' traffic.
fn read_interfaces() -> Option<HashMap<u16, ByteCounts>> {
    let mut mib = [sys::CTL_NET, sys::PF_ROUTE, 0, 0, sys::NET_RT_IFLIST2, 0];
    let table = sys::sysctl_table(&mut mib)?;
    let mut interfaces = HashMap::new();
    let mut at = 0;
    while at + OBYTES + 8 <= table.len() {
        let msg = &table[at..];
        let len = u16::from_ne_bytes([msg[MSG_LEN], msg[MSG_LEN + 1]]) as usize;
        if len == 0 {
            break;
        }
        let flags = i32::from_ne_bytes(msg[MSG_FLAGS..MSG_FLAGS + 4].try_into().ok()?);
        if msg[MSG_TYPE] == sys::RTM_IFINFO2 && flags & sys::IFF_LOOPBACK == 0 {
            let index = u16::from_ne_bytes([msg[MSG_INDEX], msg[MSG_INDEX + 1]]);
            let counts = ByteCounts {
                read: u64::from_ne_bytes(msg[IBYTES..IBYTES + 8].try_into().ok()?),
                written: u64::from_ne_bytes(msg[OBYTES..OBYTES + 8].try_into().ok()?),
            };
            interfaces.insert(index, counts);
        }
        at += len;
    }
    Some(interfaces)
}
