use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::{Error, MintDatabase};
use crate::dhke::hash_to_curve;
use crate::mint::MintKeySetInfo;
use crate::nuts::{BlindSignature, CurrencyUnit, Id, MintInfo, Proof, Proofs, PublicKey};
use crate::secret::Secret;
use crate::types::{MeltQuote, MintQuote};

#[derive(Debug, Clone)]
pub struct MintMemoryDatabase {
    mint_info: Arc<Mutex<MintInfo>>,
    active_keysets: Arc<Mutex<HashMap<CurrencyUnit, Id>>>,
    keysets: Arc<Mutex<HashMap<Id, MintKeySetInfo>>>,
    mint_quotes: Arc<Mutex<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<Mutex<HashMap<String, MeltQuote>>>,
    pending_proofs: Arc<Mutex<HashMap<[u8; 33], Proof>>>,
    spent_proofs: Arc<Mutex<HashMap<[u8; 33], Proof>>>,
    blinded_signatures: Arc<Mutex<HashMap<[u8; 33], BlindSignature>>>,
}

impl MintMemoryDatabase {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mint_info: MintInfo,
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        blinded_signatures: HashMap<[u8; 33], BlindSignature>,
    ) -> Result<Self, Error> {
        Ok(Self {
            mint_info: Arc::new(Mutex::new(mint_info)),
            active_keysets: Arc::new(Mutex::new(active_keysets)),
            keysets: Arc::new(Mutex::new(keysets.into_iter().map(|k| (k.id, k)).collect())),
            mint_quotes: Arc::new(Mutex::new(
                mint_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            melt_quotes: Arc::new(Mutex::new(
                melt_quotes.into_iter().map(|q| (q.id.clone(), q)).collect(),
            )),
            pending_proofs: Arc::new(Mutex::new(
                pending_proofs
                    .into_iter()
                    .map(|p| (hash_to_curve(&p.secret.to_bytes()).unwrap().to_bytes(), p))
                    .collect(),
            )),
            spent_proofs: Arc::new(Mutex::new(
                spent_proofs
                    .into_iter()
                    .map(|p| (hash_to_curve(&p.secret.to_bytes()).unwrap().to_bytes(), p))
                    .collect(),
            )),
            blinded_signatures: Arc::new(Mutex::new(blinded_signatures)),
        })
    }
}

#[async_trait]
impl MintDatabase for MintMemoryDatabase {
    type Err = Error;

    async fn set_mint_info(&self, mint_info: &MintInfo) -> Result<(), Self::Err> {
        let mut mi = self.mint_info.lock().await;
        *mi = mint_info.clone();
        Ok(())
    }
    async fn get_mint_info(&self) -> Result<MintInfo, Self::Err> {
        Ok(self.mint_info.lock().await.clone())
    }
    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Self::Err> {
        self.active_keysets.lock().await.insert(unit, id);
        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Self::Err> {
        Ok(self.active_keysets.lock().await.get(unit).cloned())
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Self::Err> {
        Ok(self.active_keysets.lock().await.clone())
    }

    async fn add_keyset_info(&self, keyset: MintKeySetInfo) -> Result<(), Error> {
        self.keysets.lock().await.insert(keyset.id, keyset);
        Ok(())
    }

    async fn get_keyset_info(&self, keyset_id: &Id) -> Result<Option<MintKeySetInfo>, Error> {
        Ok(self.keysets.lock().await.get(keyset_id).cloned())
    }

    async fn get_keyset_infos(&self) -> Result<Vec<MintKeySetInfo>, Error> {
        Ok(self.keysets.lock().await.values().cloned().collect())
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.mint_quotes
            .lock()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Self::Err> {
        Ok(self.mint_quotes.lock().await.get(quote_id).cloned())
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Self::Err> {
        Ok(self.mint_quotes.lock().await.values().cloned().collect())
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.mint_quotes.lock().await.remove(quote_id);

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Self::Err> {
        self.melt_quotes
            .lock()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Self::Err> {
        Ok(self.melt_quotes.lock().await.get(quote_id).cloned())
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Self::Err> {
        Ok(self.melt_quotes.lock().await.values().cloned().collect())
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Self::Err> {
        self.melt_quotes.lock().await.remove(quote_id);

        Ok(())
    }

    async fn add_spent_proof(&self, proof: Proof) -> Result<(), Self::Err> {
        let secret_point = hash_to_curve(&proof.secret.to_bytes())?;
        self.spent_proofs
            .lock()
            .await
            .insert(secret_point.to_bytes(), proof);
        Ok(())
    }

    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Self::Err> {
        Ok(self
            .spent_proofs
            .lock()
            .await
            .get(&hash_to_curve(&secret.to_bytes())?.to_bytes())
            .cloned())
    }

    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        Ok(self.spent_proofs.lock().await.get(&y.to_bytes()).cloned())
    }

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Self::Err> {
        self.pending_proofs
            .lock()
            .await
            .insert(hash_to_curve(&proof.secret.to_bytes())?.to_bytes(), proof);
        Ok(())
    }

    async fn get_pending_proof_by_secret(
        &self,
        secret: &Secret,
    ) -> Result<Option<Proof>, Self::Err> {
        let secret_point = hash_to_curve(&secret.to_bytes())?;
        Ok(self
            .pending_proofs
            .lock()
            .await
            .get(&secret_point.to_bytes())
            .cloned())
    }

    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Self::Err> {
        Ok(self.pending_proofs.lock().await.get(&y.to_bytes()).cloned())
    }

    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Self::Err> {
        let secret_point = hash_to_curve(&secret.to_bytes())?;
        self.pending_proofs
            .lock()
            .await
            .remove(&secret_point.to_bytes());
        Ok(())
    }

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindSignature,
    ) -> Result<(), Self::Err> {
        self.blinded_signatures
            .lock()
            .await
            .insert(blinded_message.to_bytes(), blinded_signature);
        Ok(())
    }

    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindSignature>, Self::Err> {
        Ok(self
            .blinded_signatures
            .lock()
            .await
            .get(&blinded_message.to_bytes())
            .cloned())
    }

    async fn get_blinded_signatures(
        &self,
        blinded_messages: Vec<PublicKey>,
    ) -> Result<Vec<Option<BlindSignature>>, Self::Err> {
        let mut signatures = Vec::with_capacity(blinded_messages.len());

        let blinded_signatures = self.blinded_signatures.lock().await;

        for blinded_message in blinded_messages {
            let signature = blinded_signatures.get(&blinded_message.to_bytes()).cloned();

            signatures.push(signature)
        }

        Ok(signatures)
    }
}
