#![no_main]

use cdk_common::federation::FederationTransactionProposal;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(proposal) = FederationTransactionProposal::from_json_slice(data) else {
        return;
    };

    let _ = proposal.canonical_bytes();
    let _ = proposal.proposal_id();
    let _ = proposal.conflict_keys();
});
