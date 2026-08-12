//! Deterministic publication order for identical wallet-fanned operations.

use std::time::Duration;

use cdk::federation::{
    FederationOperationId, FederationOperationKind, FederationSubmitOperationResponse, MemberId,
};

use super::FederationNodeRuntime;

const PUBLISHER_FALLBACK_SLOT: Duration = Duration::from_secs(3);

pub(crate) fn coordinates(kind: FederationOperationKind) -> bool {
    matches!(
        kind,
        FederationOperationKind::Mint
            | FederationOperationKind::Melt
            | FederationOperationKind::Swap
    )
}

pub(crate) fn publisher_rank(
    members: impl IntoIterator<Item = MemberId>,
    operation_id: FederationOperationId,
    member: MemberId,
) -> Option<usize> {
    let mut members = members.into_iter().collect::<Vec<_>>();
    members.sort_unstable();
    if members.is_empty() {
        return None;
    }
    let member_index = members.iter().position(|candidate| *candidate == member)?;

    let operation_prefix = u64::from_be_bytes(
        operation_id.to_bytes()[..8]
            .try_into()
            .expect("operation ID always contains eight prefix bytes"),
    );
    let member_count = u64::try_from(members.len()).ok()?;
    let first_index = usize::try_from(operation_prefix % member_count).ok()?;
    Some(if member_index >= first_index {
        member_index - first_index
    } else {
        members.len() - first_index + member_index
    })
}

pub(crate) fn prior_publisher_wait(rank: usize) -> Option<Duration> {
    let rank = u32::try_from(rank).unwrap_or(u32::MAX);
    (rank > 0).then(|| PUBLISHER_FALLBACK_SLOT.saturating_mul(rank))
}

pub(crate) async fn wait_for_prior_publisher(
    runtime: &FederationNodeRuntime,
    operation_id: FederationOperationId,
    delay: Duration,
) -> Result<Option<Box<FederationSubmitOperationResponse>>, cdk::Error> {
    match tokio::time::timeout(delay, runtime.wait_for_operation_result(operation_id)).await {
        Ok(result) => Ok(result?.map(Box::new)),
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cdk::federation::{FederationOperationId, FederationOperationKind, MemberId};

    use super::{coordinates, prior_publisher_wait, publisher_rank};

    fn member(id: u16) -> MemberId {
        MemberId::new(id).expect("member ID")
    }

    fn members() -> impl Iterator<Item = MemberId> {
        (1..=5).map(member)
    }

    #[test]
    fn only_identical_wallet_value_operations_are_coordinated() {
        assert!(coordinates(FederationOperationKind::Mint));
        assert!(coordinates(FederationOperationKind::Melt));
        assert!(coordinates(FederationOperationKind::Swap));
        assert!(!coordinates(FederationOperationKind::MintQuote));
        assert!(!coordinates(FederationOperationKind::MintQuotePayment));
        assert!(!coordinates(FederationOperationKind::Application));
        assert!(!coordinates(FederationOperationKind::ChainObservation));
    }

    #[test]
    fn operation_id_rotates_one_complete_deterministic_publisher_order() {
        let operation_id = FederationOperationId::from_bytes([0; 32]);
        assert_eq!(
            (1..=5)
                .map(|id| publisher_rank(members(), operation_id, member(id)))
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(2), Some(3), Some(4)]
        );

        let mut rotated_id = [0; 32];
        rotated_id[7] = 3;
        let rotated_id = FederationOperationId::from_bytes(rotated_id);
        assert_eq!(
            (1..=5)
                .map(|id| publisher_rank(members(), rotated_id, member(id)))
                .collect::<Vec<_>>(),
            vec![Some(2), Some(3), Some(4), Some(0), Some(1)]
        );
        assert_eq!(publisher_rank(members(), rotated_id, member(6)), None);

        let mut wide_id = [0; 32];
        wide_id[..8].copy_from_slice(&(u64::MAX - 1).to_be_bytes());
        let wide_id = FederationOperationId::from_bytes(wide_id);
        assert_eq!(
            (1..=5)
                .map(|id| publisher_rank(members(), wide_id, member(id)))
                .collect::<Vec<_>>(),
            vec![Some(1), Some(2), Some(3), Some(4), Some(0)]
        );
    }

    #[test]
    fn fallback_slots_are_short_ranked_and_bounded() {
        assert_eq!(prior_publisher_wait(0), None);
        assert_eq!(prior_publisher_wait(1), Some(Duration::from_secs(3)));
        assert_eq!(prior_publisher_wait(2), Some(Duration::from_secs(6)));
        assert_eq!(prior_publisher_wait(3), Some(Duration::from_secs(9)));
    }
}
