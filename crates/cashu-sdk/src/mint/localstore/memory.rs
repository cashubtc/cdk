use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cashu::dhke::hash_to_curve;
use cashu::nuts::nut02::mint::KeySet;
use cashu::nuts::{BlindedSignature, CurrencyUnit, Id, MintInfo, Proof, Proofs, PublicKey};
use cashu::secret::Secret;
use cashu::types::{MeltQuote, MintQuote};
use tokio::sync::Mutex;

use super::{Error, LocalStore};

#[derive(Debug, Clone)]
pub struct MemoryLocalStore {
    mint_info: Arc<Mutex<MintInfo>>,
    active_keysets: Arc<Mutex<HashMap<CurrencyUnit, Id>>>,
    keysets: Arc<Mutex<HashMap<Id, KeySet>>>,
    mint_quotes: Arc<Mutex<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<Mutex<HashMap<String, MeltQuote>>>,
    pending_proofs: Arc<Mutex<HashMap<Vec<u8>, Proof>>>,
    spent_proofs: Arc<Mutex<HashMap<Vec<u8>, Proof>>>,
    blinded_signatures: Arc<Mutex<HashMap<Box<[u8]>, BlindedSignature>>>,
}

impl MemoryLocalStore {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mint_info: MintInfo,
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<KeySet>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        blinded_signatures: HashMap<Box<[u8]>, BlindedSignature>,
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
                    .map(|p| {
                        (
                            hash_to_curve(&p.secret.to_bytes())
                                .unwrap()
                                .to_sec1_bytes()
                                .to_vec(),
                            p,
                        )
                    })
                    .collect(),
            )),
            spent_proofs: Arc::new(Mutex::new(
                spent_proofs
                    .into_iter()
                    .map(|p| {
                        (
                            hash_to_curve(&p.secret.to_bytes())
                                .unwrap()
                                .to_sec1_bytes()
                                .to_vec(),
                            p,
                        )
                    })
                    .collect(),
            )),
            blinded_signatures: Arc::new(Mutex::new(blinded_signatures)),
        })
    }
}

#[async_trait]
impl LocalStore for MemoryLocalStore {
    async fn set_mint_info(&self, mint_info: &MintInfo) -> Result<(), Error> {
        let mut mi = self.mint_info.lock().await;
        *mi = mint_info.clone();
        Ok(())
    }
    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.mint_info.lock().await.clone())
    }
    async fn add_active_keyset(&self, unit: CurrencyUnit, id: Id) -> Result<(), Error> {
        self.active_keysets.lock().await.insert(unit, id);
        Ok(())
    }

    async fn get_active_keyset_id(&self, unit: &CurrencyUnit) -> Result<Option<Id>, Error> {
        Ok(self.active_keysets.lock().await.get(unit).cloned())
    }

    async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Error> {
        Ok(self.active_keysets.lock().await.clone())
    }

    async fn add_keyset(&self, keyset: KeySet) -> Result<(), Error> {
        self.keysets.lock().await.insert(keyset.id, keyset);
        Ok(())
    }

    async fn get_keyset(&self, keyset_id: &Id) -> Result<Option<KeySet>, Error> {
        Ok(self.keysets.lock().await.get(keyset_id).cloned())
    }

    async fn get_keysets(&self) -> Result<Vec<KeySet>, Error> {
        Ok(self.keysets.lock().await.values().cloned().collect())
    }

    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), Error> {
        self.mint_quotes
            .lock()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_mint_quote(&self, quote_id: &str) -> Result<Option<MintQuote>, Error> {
        Ok(self.mint_quotes.lock().await.get(quote_id).cloned())
    }

    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, Error> {
        Ok(self.mint_quotes.lock().await.values().cloned().collect())
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.mint_quotes.lock().await.remove(quote_id);

        Ok(())
    }

    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), Error> {
        self.melt_quotes
            .lock()
            .await
            .insert(quote.id.clone(), quote);
        Ok(())
    }

    async fn get_melt_quote(&self, quote_id: &str) -> Result<Option<MeltQuote>, Error> {
        Ok(self.melt_quotes.lock().await.get(quote_id).cloned())
    }

    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, Error> {
        Ok(self.melt_quotes.lock().await.values().cloned().collect())
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), Error> {
        self.melt_quotes.lock().await.remove(quote_id);

        Ok(())
    }

    async fn add_spent_proof(&self, proof: Proof) -> Result<(), Error> {
        let secret_point = hash_to_curve(&proof.secret.to_bytes())?;
        self.spent_proofs
            .lock()
            .await
            .insert(secret_point.to_sec1_bytes().to_vec(), proof);
        Ok(())
    }

    async fn get_spent_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Error> {
        Ok(self
            .spent_proofs
            .lock()
            .await
            .get(&hash_to_curve(&secret.to_bytes())?.to_sec1_bytes().to_vec())
            .cloned())
    }

    async fn get_spent_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Error> {
        Ok(self
            .spent_proofs
            .lock()
            .await
            .get(&y.to_bytes().to_vec())
            .cloned())
    }

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Error> {
        self.pending_proofs.lock().await.insert(
            hash_to_curve(&proof.secret.to_bytes())?
                .to_sec1_bytes()
                .to_vec(),
            proof,
        );
        Ok(())
    }

    async fn get_pending_proof_by_secret(&self, secret: &Secret) -> Result<Option<Proof>, Error> {
        let secret_point = hash_to_curve(&secret.to_bytes())?;
        Ok(self
            .pending_proofs
            .lock()
            .await
            .get(&secret_point.to_sec1_bytes().to_vec())
            .cloned())
    }

    async fn get_pending_proof_by_y(&self, y: &PublicKey) -> Result<Option<Proof>, Error> {
        Ok(self
            .pending_proofs
            .lock()
            .await
            .get(&y.to_bytes().to_vec())
            .cloned())
    }

    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Error> {
        let secret_point = hash_to_curve(&secret.to_bytes())?;
        self.pending_proofs
            .lock()
            .await
            .remove(&secret_point.to_sec1_bytes().to_vec());
        Ok(())
    }

    async fn add_blinded_signature(
        &self,
        blinded_message: PublicKey,
        blinded_signature: BlindedSignature,
    ) -> Result<(), Error> {
        self.blinded_signatures
            .lock()
            .await
            .insert(blinded_message.to_bytes(), blinded_signature);
        Ok(())
    }

    async fn get_blinded_signature(
        &self,
        blinded_message: &PublicKey,
    ) -> Result<Option<BlindedSignature>, Error> {
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
    ) -> Result<Vec<Option<BlindedSignature>>, Error> {
        let mut signatures = Vec::with_capacity(blinded_messages.len());

        let blinded_signatures = self.blinded_signatures.lock().await;

        for blinded_message in blinded_messages {
            let signature = blinded_signatures.get(&blinded_message.to_bytes()).cloned();

            signatures.push(signature)
        }

        Ok(signatures)
    }
}
