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

pub(super) fn is_selected_bark_outgoing_terminal_observer(
    members: impl IntoIterator<Item = MemberId>,
    actor: MemberId,
    member: MemberId,
    failed_observer_count: usize,
    failure_threshold: usize,
    fallback_round: u32,
) -> bool {
    let (required, fallback_round) = if failed_observer_count == 0 {
        // The actor owns the healthy terminal transition. After its durable
        // fallback deadline, admit exactly one independent member that can
        // discover terminal failure without multiplying healthy-path polls.
        (1, fallback_round.min(1))
    } else {
        // One failure report is not authority. Expand to the existing
        // independent-observation threshold and retain its liveness fallback.
        (failure_threshold, fallback_round)
    };
    is_selected_bark_observer(members, actor, member, required, fallback_round)
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
        is_selected_bark_observer, is_selected_bark_outgoing_terminal_observer,
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
    fn outgoing_terminal_selection_starts_with_actor_and_one_bounded_fallback() {
        let selected = |round| {
            (1..=5)
                .filter(|id| {
                    is_selected_bark_outgoing_terminal_observer(
                        members(),
                        member(4),
                        member(*id),
                        0,
                        3,
                        round,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(selected(0), vec![4]);
        assert_eq!(selected(1), vec![4, 5]);
        assert_eq!(selected(2), vec![4, 5]);
        assert_eq!(selected(u32::MAX), vec![4, 5]);
    }

    #[test]
    fn first_failure_reactivates_threshold_selection_and_liveness_expansion() {
        let selected = |round| {
            (1..=5)
                .filter(|id| {
                    is_selected_bark_outgoing_terminal_observer(
                        members(),
                        member(4),
                        member(*id),
                        1,
                        3,
                        round,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(selected(0), vec![1, 4, 5]);
        assert_eq!(selected(1), vec![1, 2, 4, 5]);
        assert_eq!(selected(2), vec![1, 2, 3, 4, 5]);
    }
}
