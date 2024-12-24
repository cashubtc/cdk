//! Mint in memory database
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};

use crate::cdk_database::Error;
use crate::dhke::hash_to_curve;
use crate::mint::{MintAuthDatabase, MintKeySetInfo};
use crate::nuts::nut07::State;
use crate::nuts::{nut07, AuthProof, BlindSignature, Id, PublicKey};

/// Mint Memory Auth Database
#[derive(Debug, Clone, Default)]
#[allow(clippy::type_complexity)]
pub struct MintMemoryAuthDatabase {
    active_keyset: Arc<RwLock<Option<Id>>>,
    keysets: Arc<RwLock<HashMap<Id, MintKeySetInfo>>>,
    proofs: Arc<RwLock<HashMap<[u8; 33], AuthProof>>>,
    proof_state: Arc<Mutex<HashMap<[u8; 33], nut07::State>>>,
    blinded_signatures: Arc<RwLock<HashMap<[u8; 33], BlindSignature>>>,
}

impl MintMemoryAuthDatabase {
    /// Create new [`MintMemoryDatabase`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        active_keyset: Id,
        keysets: Vec<MintKeySetInfo>,
        spent_proofs: Vec<AuthProof>,
        blinded_signatures: HashMap<[u8; 33], BlindSignature>,
    ) -> Result<Self, Error> {
        let mut proofs = HashMap::new();
        let mut proof_states = HashMap::new();

        for proof in spent_proofs {
            let y = hash_to_curve(&proof.secret.to_bytes())?.to_bytes();
            proofs.insert(y, proof);
            proof_states.insert(y, State::Spent);
        }

        Ok(Self {
            active_keyset: Arc::new(RwLock::new(Some(active_keyset))),
            keysets: Arc::new(RwLock::new(
                keysets.into_iter().map(|k| (k.id, k)).collect(),
            )),
            proofs: Arc::new(RwLock::new(proofs)),
            proof_state: Arc::new(Mutex::new(proof_states)),
            blinded_signatures: Arc::new(RwLock::new(blinded_signatures)),
        })
    }
}

#[async_trait]
impl MintAuthDatabase for MintMemoryAuthDatabase {
    type Err = Error;

    async fn set_active_keyset(&self, id: Id) -> Result<(), Self::Err> {
        let mut active = self.active_keyset.write().await;

        *active = Some(id);

        Ok(())
    }

    async fn get_active_keyset_id(&self) -> Result<Option<Id>, Self::Err> {
        Ok(*self.active_keyset.read().await)
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Self::Err> {
        self.keysets.write().await.insert(keyset.id, keyset);
        Ok(())
    }

    async fn get_keyset_info(&self, keyset_id: &Id) -> Result<Option<MintKeySetInfo>, Self::Err> {
        Ok(self.keysets.read().await.get(keyset_id).cloned())
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Self::Err> {
        Ok(self.keysets.read().await.values().cloned().collect())
    }

    async fn add_proof(&self, proof: AuthProof) -> Result<(), Self::Err> {
        let mut db_proofs = self.proofs.write().await;

        let y = hash_to_curve(&proof.secret.to_bytes())?;

        let y = y.to_bytes();

        db_proofs.insert(y, proof);

        Ok(())
    }
    async fn update_proof_state(
        &self,
        y: &PublicKey,
        proof_state: State,
    ) -> Result<Option<State>, Self::Err> {
        let mut proofs_states = self.proof_state.lock().await;

        let state = proofs_states.insert(y.to_bytes(), proof_state);

        Ok(state)
    }

    async fn get_proofs_states(&self, ys: &[PublicKey]) -> Result<Vec<Option<State>>, Self::Err> {
        let proofs_states = self.proof_state.lock().await;

        let mut states = Vec::new();

        for y in ys {
            let state = proofs_states.get(&y.to_bytes()).cloned();
            states.push(state);
        }

        Ok(states)
    }

    async fn add_blind_signatures(
        &self,
        blinded_message: &[PublicKey],
        blind_signatures: &[BlindSignature],
    ) -> Result<(), Self::Err> {
        let mut current_blinded_signatures = self.blinded_signatures.write().await;

        for (blinded_message, blind_signature) in blinded_message.iter().zip(blind_signatures) {
            current_blinded_signatures.insert(blinded_message.to_bytes(), blind_signature.clone());
        }

        Ok(())
    }

    async fn get_blind_signatures(
        &self,
        blinded_messages: &[PublicKey],
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let mut signatures = Vec::with_capacity(blinded_messages.len());

        let blinded_signatures = self.blinded_signatures.read().await;

        for blinded_message in blinded_messages {
            let signature = blinded_signatures.get(&blinded_message.to_bytes()).cloned();

            signatures.push(signature)
        }

        Ok(signatures)
    }

    async fn get_blind_signatures_for_keyset(
        &self,
        keyset_id: &Id,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let blinded_signatures = self.blinded_signatures.read().await;

        Ok(blinded_signatures
            .values()
            .filter(|b| &b.keyset_id == keyset_id)
            .cloned()
            .collect())
    }
}
