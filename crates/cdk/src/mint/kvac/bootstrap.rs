use std::collections::HashSet;

use cashu_kvac::kvac::BootstrapProof;
use cashu_kvac::transcript::CashuTranscript;
use cdk_common::kvac::Error::{
    BootstrapVerificationError, InputsToProofsLengthMismatch, RequestInvalidInputLength,
};
use cdk_common::kvac::{KvacBootstrapRequest, KvacBootstrapResponse};
use tracing::instrument;

use super::super::Mint;
use crate::Error;

impl Mint {
    /// Processes a [`BootstrapRequest`].
    ///
    /// Issues MACs for zero-valued attributes
    /// so that the client might use these as inputs in further (swap/mint/melt) requests.
    #[instrument(skip_all)]
    pub async fn process_bootstrap_request(
        &self,
        bootstrap_request: KvacBootstrapRequest,
    ) -> Result<KvacBootstrapResponse, Error> {
        tracing::info!("Bootstrap has been called");

        // Length of the input vector must be 2
        // further privacy hardening
        // (if enforced at a protocol level)
        let outputs = bootstrap_request.outputs;
        if outputs.len() < 2 {
            return Err(Error::from(RequestInvalidInputLength));
        }

        let proofs = bootstrap_request.proofs;
        if outputs.len() != proofs.len() {
            return Err(Error::from(InputsToProofsLengthMismatch));
        }

        let mut keysets = vec![];
        let mut keyset_units = HashSet::with_capacity(outputs.len());
        for input in outputs.iter() {
            match self
                .localstore
                .get_kvac_keyset_info(&input.keyset_id)
                .await?
            {
                Some(keyset) => {
                    keyset_units.insert(keyset.unit.clone());
                    keysets.push(keyset);
                }
                None => {
                    tracing::error!("Bootstrap request with unknown keyset in outputs");
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
        for (input, proof) in outputs.iter().zip(proofs) {
            if !BootstrapProof::verify(&input.commitments.0, proof, &mut transcript) {
                return Err(Error::from(BootstrapVerificationError));
            }
        }

        // Proofs are verified. Issue MACs.
        // ...And prove to the client that the correct key was used.
        let mut macs = vec![];
        let mut proofs = vec![];
        let mut proving_transcript = CashuTranscript::new();
        for output in outputs.iter() {
            let (mac, proof) = self.issue_mac(output, &mut proving_transcript).await?;
            macs.push(mac);
            proofs.push(proof);
        }

        Ok(KvacBootstrapResponse { macs, proofs })
    }
}
