//! Mint in memory database

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use super::{Error, MintDatabase};
use crate::dhke::hash_to_curve;
use crate::mint::{self, MintKeySetInfo, MintQuote};
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut07::State;
use crate::nuts::{
    nut07, BlindSignature, CurrencyUnit, Id, MeltBolt11Request, MeltQuoteState, MintQuoteState,
    Proof, Proofs, PublicKey,
};
use crate::types::LnKey;

/// Mint Memory Database
#[derive(Debug, Clone, Default)]
#[allow(clippy::type_complexity)]
pub struct MintMemoryDatabase {
    active_keysets: Arc<RwLock<HashMap<CurrencyUnit, Id>>>,
    keysets: Arc<RwLock<HashMap<Id, MintKeySetInfo>>>,
    mint_quotes: Arc<RwLock<HashMap<Uuid, MintQuote>>>,
    melt_quotes: Arc<RwLock<HashMap<Uuid, mint::MeltQuote>>>,
    proofs: Arc<RwLock<HashMap<[u8; 33], Proof>>>,
    proof_state: Arc<Mutex<HashMap<[u8; 33], nut07::State>>>,
    quote_proofs: Arc<Mutex<HashMap<Uuid, Vec<PublicKey>>>>,
    blinded_signatures: Arc<RwLock<HashMap<[u8; 33], BlindSignature>>>,
    quote_signatures: Arc<RwLock<HashMap<Uuid, Vec<BlindSignature>>>>,
    melt_requests: Arc<RwLock<HashMap<Uuid, (MeltBolt11Request<Uuid>, LnKey)>>>,
}

impl MintMemoryDatabase {
    /// Create new [`MintMemoryDatabase`]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<mint::MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        quote_proofs: HashMap<Uuid, Vec<PublicKey>>,
        blinded_signatures: HashMap<[u8; 33], BlindSignature>,
        quote_signatures: HashMap<Uuid, Vec<BlindSignature>>,
        melt_request: Vec<(MeltBolt11Request<Uuid>, LnKey)>,
    ) -> Result<Self, Error> {
        let mut proofs = HashMap::new();
        let mut proof_states = HashMap::new();

        for proof in pending_proofs {
            let y = hash_to_curve(&proof.secret.to_bytes())?.to_bytes();
            proofs.insert(y, proof);
            proof_states.insert(y, State::Pending);
        }

        for proof in spent_proofs {
            let y = hash_to_curve(&proof.secret.to_bytes())?.to_bytes();
            proofs.insert(y, proof);
            proof_states.insert(y, State::Spent);
        }

        let melt_requests = melt_request
            .into_iter()
            .map(|(request, ln_key)| (request.quote, (request, ln_key)))
            .collect();

        Ok(Self {
            active_keysets: Arc::new(RwLock::new(active_keysets)),
            keysets: Arc::new(RwLock::new(
                keysets.into_iter().map(|k| (k.id, k)).collect(),
            )),
            mint_quotes: Arc::new(RwLock::new(
                mint_quotes.into_iter().map(|q| (q.id, q)).collect(),
            )),
            melt_quotes: Arc::new(RwLock::new(
                melt_quotes.into_iter().map(|q| (q.id, q)).collect(),
            )),
            proofs: Arc::new(RwLock::new(proofs)),
            proof_state: Arc::new(Mutex::new(proof_states)),
            blinded_signatures: Arc::new(RwLock::new(blinded_signatures)),
            quote_proofs: Arc::new(Mutex::new(quote_proofs)),
            quote_signatures: Arc::new(RwLock::new(quote_signatures)),
            melt_requests: Arc::new(RwLock::new(melt_requests)),
        })
    }
}

#[async_trait]
impl MintDatabase for MintMemoryDatabase {
    type Err = Error;

    async fn set_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        self.active_keysets.write().await.insert(unit, id);
        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        Ok(self.active_keysets.read().await.get(unit).cloned())
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        Ok(self.active_keysets.read().await.clone())
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

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Self::Err> {
        self.mint_quotes.write().await.insert(quote.id, quote);
        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &Uuid) -> Result<Option<MintQuote>, Self::Err> {
        Ok(self.mint_quotes.read().await.get(quote_id).cloned())
    }

    async fn update_mint_quote_state(
        &self,
        quote_id: &Uuid,
        state: MintQuoteState,
    ) -> Result<MintQuoteState, Self::Err> {
        let mut mint_quotes = self.mint_quotes.write().await;

        let mut quote = mint_quotes
            .get(quote_id)
            .cloned()
            .ok_or(Error::UnknownQuote)?;

        let current_state = quote.state;

        quote.state = state;

        mint_quotes.insert(*quote_id, quote.clone());

        Ok(current_state)
    }

    async fn get_mint_quote_by_request_lookup_id(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let quotes = self.get_mint_quotes().await?;

        let quote = quotes
            .into_iter()
            .filter(|q| q.request_lookup_id.eq(request))
            .collect::<Vec<MintQuote>>()
            .first()
            .cloned();

        Ok(quote)
    }
    async fn get_mint_quote_by_request(
        &self,
        request: &str,
    ) -> Result<Option<MintQuote>, Self::Err> {
        let quotes = self.get_mint_quotes().await?;

        let quote = quotes
            .into_iter()
            .filter(|q| q.request.eq(request))
            .collect::<Vec<MintQuote>>()
            .first()
            .cloned();

        Ok(quote)
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        Ok(self.mint_quotes.read().await.values().cloned().collect())
    }

    async fn remove_mint_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err> {
        self.mint_quotes.write().await.remove(quote_id);

        Ok(())
    }

    async fn add_melt_quote(&self, quote: mint::MeltQuote) -> Result<(), Self::Err> {
        self.melt_quotes.write().await.insert(quote.id, quote);
        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &Uuid) -> Result<Option<mint::MeltQuote>, Self::Err> {
        Ok(self.melt_quotes.read().await.get(quote_id).cloned())
    }

    async fn update_melt_quote_state(
        &self,
        quote_id: &Uuid,
        state: MeltQuoteState,
    ) -> Result<MeltQuoteState, Self::Err> {
        let mut melt_quotes = self.melt_quotes.write().await;

        let mut quote = melt_quotes
            .get(quote_id)
            .cloned()
            .ok_or(Error::UnknownQuote)?;

        let current_state = quote.state;

        quote.state = state;

        melt_quotes.insert(*quote_id, quote.clone());

        Ok(current_state)
    }

    async fn get_melt_quotes(&self) -> Result<Vec<mint::MeltQuote>, Self::Err> {
        Ok(self.melt_quotes.read().await.values().cloned().collect())
    }

    async fn remove_melt_quote(&self, quote_id: &Uuid) -> Result<(), Self::Err> {
        self.melt_quotes.write().await.remove(quote_id);

        Ok(())
    }

    async fn add_melt_request(
        &self,
        melt_request: MeltBolt11Request<Uuid>,
        ln_key: LnKey,
    ) -> Result<(), Self::Err> {
        let mut melt_requests = self.melt_requests.write().await;
        melt_requests.insert(melt_request.quote, (melt_request, ln_key));
        Ok(())
    }

    async fn get_melt_request(
        &self,
        quote_id: &Uuid,
    ) -> Result<Option<(MeltBolt11Request<Uuid>, LnKey)>, Self::Err> {
        let melt_requests = self.melt_requests.read().await;

        let melt_request = melt_requests.get(quote_id);

        Ok(melt_request.cloned())
    }

    async fn add_proofs(&self, proofs: Proofs, quote_id: Option<Uuid>) -> Result<(), Self::Err> {
        let mut db_proofs = self.proofs.write().await;

        let mut ys = Vec::with_capacity(proofs.capacity());

        for proof in proofs {
            let y = hash_to_curve(&proof.secret.to_bytes())?;
            ys.push(y);

            let y = y.to_bytes();

            db_proofs.insert(y, proof);
        }

        if let Some(quote_id) = quote_id {
            let mut db_quote_proofs = self.quote_proofs.lock().await;

            db_quote_proofs.insert(quote_id, ys);
        }

        Ok(())
    }

    async fn get_proofs_by_ys(&self, ys: &[PublicKey]) -> Result<Vec<Option<Proof>>, Self::Err> {
        let spent_proofs = self.proofs.read().await;

        let mut proofs = Vec::with_capacity(ys.len());

        for y in ys {
            let proof = spent_proofs.get(&y.to_bytes()).cloned();

            proofs.push(proof);
        }

        Ok(proofs)
    }

    async fn get_proof_ys_by_quote_id(&self, quote_id: &Uuid) -> Result<Vec<PublicKey>, Self::Err> {
        let quote_proofs = &__self.quote_proofs.lock().await;

        match quote_proofs.get(quote_id) {
            Some(ys) => Ok(ys.clone()),
            None => Ok(vec![]),
        }
    }

    async fn update_proofs_states(
        &self,
        ys: &[PublicKey],
        proof_state: State,
    ) -> Result<Vec<Option<State>>, Self::Err> {
        let mut proofs_states = self.proof_state.lock().await;

        let mut states = Vec::new();

        for y in ys {
            let state = proofs_states.insert(y.to_bytes(), proof_state);
            states.push(state);
        }

        Ok(states)
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

    async fn get_proofs_by_keyset_id(
        &self,
        keyset_id: &Id,
    ) -> Result<(Proofs, Vec<Option<State>>), Self::Err> {
        let proofs = self.proofs.read().await;

        let proofs_for_id: Proofs = proofs
            .iter()
            .filter_map(|(_, p)| match &p.keyset_id == keyset_id {
                true => Some(p),
                false => None,
            })
            .cloned()
            .collect();

        let proof_ys = proofs_for_id.ys()?;

        assert_eq!(proofs_for_id.len(), proof_ys.len());

        let states = self.get_proofs_states(&proof_ys).await?;

        Ok((proofs_for_id, states))
    }

    async fn add_blind_signatures(
        &self,
        blinded_message: &[PublicKey],
        blind_signatures: &[BlindSignature],
        quote_id: Option<Uuid>,
    ) -> Result<(), Self::Err> {
        let mut current_blinded_signatures = self.blinded_signatures.write().await;

        for (blinded_message, blind_signature) in blinded_message.iter().zip(blind_signatures) {
            current_blinded_signatures.insert(blinded_message.to_bytes(), blind_signature.clone());
        }

        if let Some(quote_id) = quote_id {
            let mut current_quote_signatures = self.quote_signatures.write().await;
            current_quote_signatures.insert(quote_id, blind_signatures.to_vec());
            let t = current_quote_signatures.get(&quote_id);
            println!("after insert: {:?}", t);
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

    /// Get [`BlindSignature`]s for quote
    async fn get_blind_signatures_for_quote(
        &self,
        quote_id: &Uuid,
    ) -> Result<Vec<BlindSignature>, Self::Err> {
        let ys = self.quote_signatures.read().await;

        Ok(ys.get(quote_id).cloned().unwrap_or_default())
    }
}
