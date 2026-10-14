use crate::snapshot::{CoreKind, CoreTier, CoreUsage};

/// Labels logical CPUs by the tier each falls in, `tiers` being in logical
/// CPU order as [`ChipInfo::tiers`](crate::snapshot::ChipInfo::tiers) holds
/// them. Cores no tier covers, as on
/// Intel Macs, count as Performance.
pub fn label_cores(usage: &[f32], tiers: &[CoreTier]) -> Vec<CoreUsage> {
    let kinds = tiers
        .iter()
        .flat_map(|tier| std::iter::repeat_n(tier.kind, tier.cores as usize))
        .chain(std::iter::repeat(CoreKind::Performance));
    usage
        .iter()
        .zip(kinds)
        .map(|(&usage, kind)| CoreUsage { kind, usage })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cores_take_their_tier_in_order_and_the_rest_count_as_performance() {
        let tiers = [
            CoreTier {
                kind: CoreKind::Efficiency,
                cores: 2,
            },
            CoreTier {
                kind: CoreKind::Super,
                cores: 1,
            },
        ];
        let kinds: Vec<CoreKind> = label_cores(&[1.0, 2.0, 3.0, 4.0], &tiers)
            .iter()
            .map(|core| core.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                CoreKind::Efficiency,
                CoreKind::Efficiency,
                CoreKind::Super,
                CoreKind::Performance
            ]
        );
        // Without tiers, as on an Intel Mac, every core is Performance.
        assert!(
            label_cores(&[1.0], &[])
                .iter()
                .all(|core| core.kind == CoreKind::Performance)
        );
    }
}
