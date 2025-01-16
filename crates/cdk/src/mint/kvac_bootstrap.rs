use std::collections::HashSet;

use cashu_kvac::{kvac::BootstrapProof, transcript::CashuTranscript};
use cdk_common::kvac::{BootstrapRequest, BootstrapResponse};
use tracing::instrument;
use crate::Error;

use super::Mint;

impl Mint {
    /// Processes a [`BootstrapRequest`].
    /// 
    /// Issues MACs for zero-valued attributes
    /// so that the client might use these as inputs in further (swap/mint/melt) requests
    #[instrument(skip_all)]
    pub async fn process_bootstrap_request(
        &self,
        bootstrap_request: BootstrapRequest
    ) -> Result<BootstrapResponse, Error> {
        tracing::info!("Bootstrap has been called");
        
        // Length of the input vector must be 2
        // further privacy hardening
        // (if enforced at a protocol level)
        let inputs = bootstrap_request.inputs;
        if inputs.len() != 2 {
            return Err(Error::RequestInvalidInputLength);
        }

        let proofs = bootstrap_request.proofs;
        if inputs.len() != proofs.len() {
            return Err(Error::InputsToProofsLengthMismatch);
        }

        let mut keysets = vec![];
        let mut keyset_units = HashSet::with_capacity(inputs.len());
        for input in inputs.iter() {
            match self.localstore.get_keyset_info(&input.keyset_id).await? {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit.clone());
                    keysets.push(keyset);
                }
                None => {
                    tracing::info!("Bootstrap request with unknown keyset in inputs");
                    return Err(Error::UnknownKeySet);
                }
            }
        }

        // Check that all proofs are the same unit
        // in the future it maybe possible to support multiple units but unsupported for
        // now
        if keyset_units.len().gt(&1) {
            tracing::error!("Only one unit is allowed in request: {:?}", keyset_units);
            return Err(Error::MultipleUnits);
        }

        let mut transcript = CashuTranscript::new();
        for (input, proof) in inputs.iter().zip(proofs) {
            if !BootstrapProof::verify(&input.coin.0, proof, &mut transcript) {
                return Err(Error::BootstrapVerificationError)
            }
        }

        // Proofs are verified. Issue MACs.
        // ...And prove to the client that the correct key was used.
        let mut macs = vec![];
        let mut proofs = vec![];
        let mut proving_transcript = CashuTranscript::new();
        for input in inputs.iter() {
            let (mac, proof) = self.issue_mac(input, &mut proving_transcript).await?;
            macs.push(mac);
            proofs.push(proof);
        }

        Ok(BootstrapResponse {
            coins: inputs,
            macs,
            proofs,
        })
    }
}