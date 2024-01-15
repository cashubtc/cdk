use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cashu::nuts::nut02::mint::KeySet;
use cashu::nuts::{CurrencyUnit, Id, Proof, Proofs};
use cashu::secret::Secret;
use cashu::types::{MeltQuote, MintQuote};
use tokio::sync::Mutex;

use super::{Error, LocalStore};

#[derive(Default, Debug, Clone)]
pub struct MemoryLocalStore {
    active_keysets: Arc<Mutex<HashMap<CurrencyUnit, Id>>>,
    keysets: Arc<Mutex<HashMap<Id, KeySet>>>,
    mint_quotes: Arc<Mutex<HashMap<String, MintQuote>>>,
    melt_quotes: Arc<Mutex<HashMap<String, MeltQuote>>>,
    pending_proofs: Arc<Mutex<HashMap<Secret, Proof>>>,
    spent_proofs: Arc<Mutex<HashMap<Secret, Proof>>>,
}

impl MemoryLocalStore {
    pub fn new(
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<KeySet>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
    ) -> Self {
        Self {
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
                    .map(|p| (p.secret.clone(), p))
                    .collect(),
            )),
            spent_proofs: Arc::new(Mutex::new(
                spent_proofs
                    .into_iter()
                    .map(|p| (p.secret.clone(), p))
                    .collect(),
            )),
        }
    }
}

#[async_trait]
impl LocalStore for MemoryLocalStore {
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
        self.spent_proofs
            .lock()
            .await
            .insert(proof.secret.clone(), proof);
        Ok(())
    }

    async fn get_spent_proof(&self, secret: &Secret) -> Result<Option<Proof>, Error> {
        Ok(self.spent_proofs.lock().await.get(secret).cloned())
    }

    async fn add_pending_proof(&self, proof: Proof) -> Result<(), Error> {
        self.pending_proofs
            .lock()
            .await
            .insert(proof.secret.clone(), proof);
        Ok(())
    }

    async fn get_pending_proof(&self, secret: &Secret) -> Result<Option<Proof>, Error> {
        Ok(self.pending_proofs.lock().await.get(secret).cloned())
    }

    async fn remove_pending_proof(&self, secret: &Secret) -> Result<(), Error> {
        self.pending_proofs.lock().await.remove(secret);
        Ok(())
    }
}
