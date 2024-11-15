use std::collections::HashSet;

use tracing::instrument;

use super::nut11::{enforce_sig_flag, EnforceSigFlag};
use super::{Id, Mint, PublicKey, SigFlag, State, SwapRequest, SwapResponse};
use crate::nuts::nut00::ProofsMethods;
use crate::Error;

impl Mint {
    /// Process Swap
    #[instrument(skip_all)]
    pub async fn process_swap_request(
        &self,
        swap_request: SwapRequest,
    ) -> Result<SwapResponse, Error> {
        let blinded_messages: Vec<PublicKey> = swap_request
            .outputs
            .iter()
            .map(|b| b.blinded_secret)
            .collect();

        if self
            .localstore
            .get_blind_signatures(&blinded_messages)
            .await?
            .iter()
            .flatten()
            .next()
            .is_some()
        {
            tracing::info!("Output has already been signed",);

            return Err(Error::BlindedMessageAlreadySigned);
        }

        let proofs_total = swap_request.input_amount()?;

        let output_total = swap_request.output_amount()?;

        let fee = self.get_proofs_fee(&swap_request.inputs).await?;

        let total_with_fee = output_total.checked_add(fee).ok_or(Error::AmountOverflow)?;

        if proofs_total != total_with_fee {
            tracing::info!(
                "Swap request unbalanced: {}, outputs {}, fee {}",
                proofs_total,
                output_total,
                fee
            );
            return Err(Error::TransactionUnbalanced(
                proofs_total.into(),
                output_total.into(),
                fee.into(),
            ));
        }

        let proof_count = swap_request.inputs.len();

        let input_ys = swap_request.inputs.ys()?;

        self.localstore
            .add_proofs(swap_request.inputs.clone(), None)
            .await?;
        self.check_ys_spendable(&input_ys, State::Pending).await?;

        // Check that there are no duplicate proofs in request
        if input_ys
            .iter()
            .collect::<HashSet<&PublicKey>>()
            .len()
            .ne(&proof_count)
        {
            self.localstore
                .update_proofs_states(&input_ys, State::Unspent)
                .await?;
            return Err(Error::DuplicateProofs);
        }

        for proof in &swap_request.inputs {
            if let Err(err) = self.verify_proof(proof).await {
                tracing::info!("Error verifying proof in swap");
                self.localstore
                    .update_proofs_states(&input_ys, State::Unspent)
                    .await?;
                return Err(err);
            }
        }

        let input_keyset_ids: HashSet<Id> =
            swap_request.inputs.iter().map(|p| p.keyset_id).collect();

        let mut keyset_units = HashSet::with_capacity(input_keyset_ids.capacity());

        for id in input_keyset_ids {
            match self.localstore.get_keyset_info(&id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::info!("Swap request with unknown keyset in inputs");
                    self.localstore
                        .update_proofs_states(&input_ys, State::Unspent)
                        .await?;
                }
            }
        }

        let output_keyset_ids: HashSet<Id> =
            swap_request.outputs.iter().map(|p| p.keyset_id).collect();

        for id in &output_keyset_ids {
            match self.localstore.get_keyset_info(id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit);
                }
                None => {
                    tracing::info!("Swap request with unknown keyset in outputs");
                    self.localstore
                        .update_proofs_states(&input_ys, State::Unspent)
                        .await?;
                }
            }
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len().gt(&1) {
            tracing::error!("Only one unit is allowed in request: {:?}", keyset_units);
            self.localstore
                .update_proofs_states(&input_ys, State::Unspent)
                .await?;
            return Err(Error::MultipleUnits);
        }

        let EnforceSigFlag {
            sig_flag,
            pubkeys,
            sigs_required,
        } = enforce_sig_flag(swap_request.inputs.clone());

        if sig_flag.eq(&SigFlag::SigAll) {
            let pubkeys = pubkeys.into_iter().collect();
            for blinded_message in &swap_request.outputs {
                if let Err(err) = blinded_message.verify_p2pk(&pubkeys, sigs_required) {
                    tracing::info!("Could not verify p2pk in swap request");
                    self.localstore
                        .update_proofs_states(&input_ys, State::Unspent)
                        .await?;
                    return Err(err.into());
                }
            }
        }

        let mut promises = Vec::with_capacity(swap_request.outputs.len());

        for blinded_message in swap_request.outputs.iter() {
            let blinded_signature = self.blind_sign(blinded_message).await?;
            promises.push(blinded_signature);
        }

        self.localstore
            .update_proofs_states(&input_ys, State::Spent)
            .await?;

        for pub_key in input_ys {
            self.pubsub_manager.proof_state((pub_key, State::Spent));
        }

        self.localstore
            .add_blind_signatures(
                &swap_request
                    .outputs
                    .iter()
                    .map(|o| o.blinded_secret)
                    .collect::<Vec<PublicKey>>(),
                &promises,
                None,
            )
            .await?;

        Ok(SwapResponse::new(promises))
    }
}
