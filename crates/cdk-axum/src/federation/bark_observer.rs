//! Deterministic, threshold-sized Bark observer selection.

use cdk::federation::MemberId;

use super::payment_schedule::FederationPaymentAdvancement;

pub(super) fn is_selected_bark_observer(
    members: impl IntoIterator<Item = MemberId>,
    actor: MemberId,
    member: MemberId,
    required: usize,
    fallback_round: u32,
) -> bool {
    let members = members.into_iter().collect::<Vec<_>>();
    let Some(actor_index) = members.iter().position(|candidate| *candidate == actor) else {
        return false;
    };
    let Some(member_index) = members.iter().position(|candidate| *candidate == member) else {
        return false;
    };
    if required == 0 || members.is_empty() {
        return false;
    }

    let rank = if member_index >= actor_index {
        member_index - actor_index
    } else {
        members.len() - actor_index + member_index
    };
    let fallback = usize::try_from(fallback_round).unwrap_or(usize::MAX);
    rank < required.saturating_add(fallback).min(members.len())
}

pub(super) fn required_bark_result_observers(
    signature_threshold: u16,
    payment_observation_threshold: u16,
) -> usize {
    usize::from(signature_threshold).max(usize::from(payment_observation_threshold))
}

pub(super) fn bark_outgoing_terminal_pending_wait(
    actor: MemberId,
    member: MemberId,
    failed_observer_count: usize,
    fallback_round: u32,
) -> FederationPaymentAdvancement {
    if failed_observer_count == 0 && member == actor && fallback_round == 0 {
        FederationPaymentAdvancement::AwaitingObserverFallback
    } else {
        FederationPaymentAdvancement::AwaitingExternalEvidence
    }
}

pub(super) fn unselected_bark_observer_wait(
    matching_evidence_present: bool,
) -> FederationPaymentAdvancement {
    if matching_evidence_present {
        FederationPaymentAdvancement::AwaitingObserverFallback
    } else {
        FederationPaymentAdvancement::AwaitingFederationEvidence
    }
}

#[cfg(test)]
mod tests {
    use cdk::federation::MemberId;

    use super::{
        bark_outgoing_terminal_pending_wait, is_selected_bark_observer,
        required_bark_result_observers, unselected_bark_observer_wait,
    };
    use crate::federation::payment_schedule::FederationPaymentAdvancement;

    fn member(id: u16) -> MemberId {
        MemberId::new(id).expect("member ID")
    }

    fn members() -> impl Iterator<Item = MemberId> {
        (1..=5).map(member)
    }

    #[test]
    fn selection_is_actor_first_and_expands_one_member_per_round() {
        let selected = |round| {
            (1..=5)
                .filter(|id| is_selected_bark_observer(members(), member(4), member(*id), 3, round))
                .collect::<Vec<_>>()
        };
        assert_eq!(selected(0), vec![1, 4, 5]);
        assert_eq!(selected(1), vec![1, 2, 4, 5]);
        assert_eq!(selected(2), vec![1, 2, 3, 4, 5]);
        assert_eq!(selected(u32::MAX), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn terminal_results_start_at_the_stronger_threshold() {
        assert_eq!(required_bark_result_observers(3, 4), 4);
        assert!(is_selected_bark_observer(
            members(),
            member(1),
            member(4),
            4,
            0,
        ));
        assert!(!is_selected_bark_observer(
            members(),
            member(1),
            member(5),
            4,
            0,
        ));
        assert!(is_selected_bark_observer(
            members(),
            member(1),
            member(5),
            4,
            1,
        ));
    }

    #[test]
    fn fallback_timer_starts_only_after_matching_evidence_exists() {
        assert_eq!(
            unselected_bark_observer_wait(false),
            FederationPaymentAdvancement::AwaitingFederationEvidence
        );
        assert_eq!(
            unselected_bark_observer_wait(true),
            FederationPaymentAdvancement::AwaitingObserverFallback
        );
    }

    #[test]
    fn actor_pending_yields_the_terminal_lease_once() {
        assert_eq!(
            bark_outgoing_terminal_pending_wait(member(4), member(4), 0, 0),
            FederationPaymentAdvancement::AwaitingObserverFallback
        );
        assert_eq!(
            bark_outgoing_terminal_pending_wait(member(4), member(5), 0, 1),
            FederationPaymentAdvancement::AwaitingExternalEvidence
        );
        assert_eq!(
            bark_outgoing_terminal_pending_wait(member(4), member(4), 1, 0),
            FederationPaymentAdvancement::AwaitingExternalEvidence
        );
    }
}
