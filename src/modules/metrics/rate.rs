use std::collections::HashMap;
use std::hash::Hash;
use std::time::Instant;

use crate::modules::metrics::snapshot::Throughput;

/// Bytes one device has read and written since it appeared.
#[derive(Debug, Clone, Copy, Default)]
pub struct ByteCounts {
    pub read: u64,
    pub written: u64,
}

/// Per-second byte rates from reads of per-device counters, keyed by device.
/// Only devices in both reads of an interval count toward it, so one that
/// appears, disappears or misses a read can't pass its since-boot total off as
/// an interval's traffic, or cancel out the others'.
pub struct ByteRates<K> {
    previous: Option<(Instant, HashMap<K, ByteCounts>)>,
}

impl<K> Default for ByteRates<K> {
    fn default() -> Self {
        Self { previous: None }
    }
}

impl<K: Eq + Hash> ByteRates<K> {
    /// Rates since the last good read. `None` on the first read, which only
    /// sets the baseline, and when `current` is `None` because the read failed.
    pub fn sample(
        &mut self,
        now: Instant,
        current: Option<HashMap<K, ByteCounts>>,
    ) -> Option<Throughput> {
        // A failed read keeps the baseline, so the next sample spans the gap.
        let current = current?;
        let rates = self.previous.as_ref().map(|(at, before)| {
            let secs = now.duration_since(*at).as_secs_f64().max(1e-3);
            let bytes = moved(before, &current);
            Throughput {
                read: bytes.read as f64 / secs,
                write: bytes.written as f64 / secs,
            }
        });
        self.previous = Some((now, current));
        rates
    }
}

/// Bytes moved between two reads by the devices present in both. A counter
/// that went backwards, its device replaced under the same key, moved nothing.
fn moved<K: Eq + Hash>(
    before: &HashMap<K, ByteCounts>,
    after: &HashMap<K, ByteCounts>,
) -> ByteCounts {
    after
        .iter()
        .filter_map(|(device, after)| Some((before.get(device)?, after)))
        .fold(ByteCounts::default(), |sum, (before, after)| ByteCounts {
            read: sum
                .read
                .saturating_add(after.read.saturating_sub(before.read)),
            written: sum
                .written
                .saturating_add(after.written.saturating_sub(before.written)),
        })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn counts(read: u64, written: u64) -> ByteCounts {
        ByteCounts { read, written }
    }

    #[test]
    fn rates_are_bytes_per_second_since_the_last_read() {
        let mut rates = ByteRates::default();
        let start = Instant::now();
        // Since-boot counters are not an interval, so the first read only sets the baseline.
        let first = rates.sample(start, Some(HashMap::from([(1, counts(1000, 500))])));
        assert!(first.is_none());
        let later = start + Duration::from_secs(2);
        let second = rates.sample(later, Some(HashMap::from([(1, counts(5000, 2500))])));
        let second = second.expect("a baseline to measure against");
        assert_eq!((second.read, second.write), (2000.0, 1000.0));
    }

    #[test]
    fn a_device_missing_from_one_read_does_not_spike_the_next() {
        let mut rates = ByteRates::default();
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        let ssd = counts(80_000_000_000, 0);
        rates.sample(at(0), Some(HashMap::from([(1, counts(1000, 0)), (2, ssd)])));
        // Device 2's statistics can't be read this time.
        rates.sample(at(1), Some(HashMap::from([(1, counts(2000, 0))])));
        let back = HashMap::from([(1, counts(3000, 0)), (2, counts(80_000_000_500, 0))]);
        // Device 2 has no baseline in the previous read, so only device 1's 1000 bytes count,
        // not its 80 GB since boot.
        assert_eq!(
            rates.sample(at(2), Some(back)).map(|r| r.read),
            Some(1000.0)
        );
    }

    #[test]
    fn a_detached_device_does_not_hide_the_others_traffic() {
        let mut rates = ByteRates::default();
        let start = Instant::now();
        let external = counts(0, 5_000_000_000);
        rates.sample(
            start,
            Some(HashMap::from([(1, counts(0, 1000)), (2, external)])),
        );
        // Device 2 is ejected; summing would net device 1's 500 bytes against its 5 GB.
        let later = start + Duration::from_secs(1);
        let after = rates.sample(later, Some(HashMap::from([(1, counts(0, 1500))])));
        assert_eq!(after.map(|r| r.write), Some(500.0));
    }

    #[test]
    fn a_failed_read_keeps_the_baseline() {
        let mut rates = ByteRates::default();
        let start = Instant::now();
        let at = |secs| start + Duration::from_secs(secs);
        rates.sample(at(0), Some(HashMap::from([(1, counts(1000, 0))])));
        assert!(rates.sample(at(1), None).is_none());
        // 4000 bytes over the 2 seconds since the last good read.
        let next = rates.sample(at(2), Some(HashMap::from([(1, counts(5000, 0))])));
        assert_eq!(next.map(|r| r.read), Some(2000.0));
    }

    #[test]
    fn deltas_too_large_to_sum_saturate_instead_of_overflowing() {
        let mut rates = ByteRates::default();
        let start = Instant::now();
        let zeroed = HashMap::from([(1, counts(0, 0)), (2, counts(0, 0))]);
        rates.sample(start, Some(zeroed));
        // Only a misbehaving driver could report these, but it must not panic the sampler.
        let maxed = HashMap::from([(1, counts(u64::MAX, 0)), (2, counts(2, 0))]);
        let later = start + Duration::from_secs(1);
        // Saturated at u64::MAX bytes; a wrapping sum would report 1 byte.
        let read = rates.sample(later, Some(maxed)).map(|r| r.read);
        assert_eq!(read, Some(1.8446744073709552e19));
    }
}
