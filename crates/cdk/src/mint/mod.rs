//! Cashu Mint

use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::{DerivationPath, Xpriv};
use bitcoin::secp256k1;
use cdk_common::common::{PaymentProcessorKey, QuoteTTL};
#[cfg(feature = "auth")]
use cdk_common::database::MintAuthDatabase;
use cdk_common::database::{self, MintDatabase};
use cdk_signatory::signatory::Signatory;
use futures::StreamExt;
#[cfg(feature = "auth")]
use nut21::ProtectedEndpoint;
use subscription::PubSubManager;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tracing::instrument;
use uuid::Uuid;

use crate::cdk_payment::{self, MintPayment};
use crate::error::Error;
use crate::fees::calculate_fee;
use crate::nuts::*;
use crate::util::unix_time;
use crate::Amount;
#[cfg(feature = "auth")]
use crate::OidcClient;

#[cfg(feature = "auth")]
pub(crate) mod auth;
mod builder;
mod check_spendable;
mod issue;
mod keysets;
mod ln;
mod melt;
mod start_up_check;
pub mod subscription;
mod swap;
mod verification;

pub use builder::{MintBuilder, MintMeltLimits};
pub use cdk_common::mint::{MeltQuote, MintKeySetInfo, MintQuote};
pub use verification::Verification;

/// Cashu Mint
#[derive(Clone)]
pub struct Mint {
    /// Signatory backend.
    ///
    /// It is mainly implemented in the cdk-signatory crate, and it can be embedded in the mint or
    /// it can be a gRPC client to a remote signatory server.
    pub signatory: Arc<dyn Signatory + Send + Sync>,
    /// Mint Storage backend
    pub localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
    /// Auth Storage backend (only available with auth feature)
    #[cfg(feature = "auth")]
    pub auth_localstore: Option<Arc<dyn MintAuthDatabase<Err = database::Error> + Send + Sync>>,
    /// Ln backends for mint
    pub ln:
        HashMap<PaymentProcessorKey, Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>>,
    /// Subscription manager
    pub pubsub_manager: Arc<PubSubManager>,
    #[cfg(feature = "auth")]
    oidc_client: Option<OidcClient>,
}

impl Mint {
    /// Get the payment processor for the given unit and payment method
    pub fn get_payment_processor(
        &self,
        unit: CurrencyUnit,
        payment_method: PaymentMethod,
    ) -> Result<Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>, Error> {
        let key = PaymentProcessorKey::new(unit.clone(), payment_method.clone());
        self.ln.get(&key).cloned().ok_or_else(|| {
            tracing::info!(
                "No payment processor set for pair {}, {}",
                unit,
                payment_method
            );
            Error::UnsupportedUnit
        })
    }

    /// Create new [`Mint`] without authentication
    pub async fn new(
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
        ln: HashMap<
            PaymentProcessorKey,
            Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
        >,
    ) -> Result<Self, Error> {
        Self::new_internal(
            signatory,
            localstore,
            #[cfg(feature = "auth")]
            None,
            ln,
            #[cfg(feature = "auth")]
            None,
        )
        .await
    }

    /// Create new [`Mint`] with authentication support
    #[cfg(feature = "auth")]
    pub async fn new_with_auth(
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
        auth_localstore: Arc<dyn MintAuthDatabase<Err = database::Error> + Send + Sync>,
        ln: HashMap<
            PaymentProcessorKey,
            Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
        >,
        open_id_discovery: String,
    ) -> Result<Self, Error> {
        Self::new_internal(
            signatory,
            localstore,
            Some(auth_localstore),
            ln,
            Some(open_id_discovery),
        )
        .await
    }

    /// Internal function to create a new [`Mint`] with shared logic
    #[inline]
    async fn new_internal(
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
        #[cfg(feature = "auth")] auth_localstore: Option<
            Arc<dyn database::MintAuthDatabase<Err = database::Error> + Send + Sync>,
        >,
        ln: HashMap<
            PaymentProcessorKey,
            Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
        >,
        #[cfg(feature = "auth")] open_id_discovery: Option<String>,
    ) -> Result<Self, Error> {
        #[cfg(feature = "auth")]
        let oidc_client =
            open_id_discovery.map(|openid_discovery| OidcClient::new(openid_discovery.clone()));

        Ok(Self {
            signatory,
            pubsub_manager: Arc::new(localstore.clone().into()),
            localstore,
            #[cfg(feature = "auth")]
            oidc_client,
            ln,
            #[cfg(feature = "auth")]
            auth_localstore,
        })
    }

    /// Get mint info
    #[instrument(skip_all)]
    pub async fn mint_info(&self) -> Result<MintInfo, Error> {
        let mint_info = self.localstore.get_mint_info().await?;

        #[cfg(feature = "auth")]
        let mint_info = if let Some(auth_db) = self.auth_localstore.as_ref() {
            let mut mint_info = mint_info;
            let auth_endpoints = auth_db.get_auth_for_endpoints().await?;

            let mut clear_auth_endpoints: Vec<ProtectedEndpoint> = vec![];
            let mut blind_auth_endpoints: Vec<ProtectedEndpoint> = vec![];

            for (endpoint, auth) in auth_endpoints {
                match auth {
                    Some(AuthRequired::Clear) => {
                        clear_auth_endpoints.push(endpoint);
                    }
                    Some(AuthRequired::Blind) => {
                        blind_auth_endpoints.push(endpoint);
                    }
                    None => (),
                }
            }

            mint_info.nuts.nut21 = mint_info.nuts.nut21.map(|mut a| {
                a.protected_endpoints = clear_auth_endpoints;
                a
            });

            mint_info.nuts.nut22 = mint_info.nuts.nut22.map(|mut a| {
                a.protected_endpoints = blind_auth_endpoints;
                a
            });
            mint_info
        } else {
            mint_info
        };

        Ok(mint_info)
    }

    /// Set mint info
    #[instrument(skip_all)]
    pub async fn set_mint_info(&self, mint_info: MintInfo) -> Result<(), Error> {
        Ok(self.localstore.set_mint_info(mint_info).await?)
    }

    /// Get quote ttl
    #[instrument(skip_all)]
    pub async fn quote_ttl(&self) -> Result<QuoteTTL, Error> {
        Ok(self.localstore.get_quote_ttl().await?)
    }

    /// Set quote ttl
    #[instrument(skip_all)]
    pub async fn set_quote_ttl(&self, quote_ttl: QuoteTTL) -> Result<(), Error> {
        Ok(self.localstore.set_quote_ttl(quote_ttl).await?)
    }

    /// Wait for any invoice to be paid
    /// For each backend starts a task that waits for any invoice to be paid
    /// Once invoice is paid mint quote status is updated
    #[instrument(skip_all)]
    pub async fn wait_for_paid_invoices(&self, shutdown: Arc<Notify>) -> Result<(), Error> {
        let mint_arc = Arc::new(self.clone());

        let mut join_set = JoinSet::new();

        for (key, ln) in self.ln.iter() {
            if !ln.is_wait_invoice_active() {
                tracing::info!("Wait payment for {:?} inactive starting.", key);
                let mint = Arc::clone(&mint_arc);
                let ln = Arc::clone(ln);
                let shutdown = Arc::clone(&shutdown);
                let key = key.clone();
                join_set.spawn(async move {
            loop {
                tracing::info!("Restarting wait for: {:?}", key);
                tokio::select! {
                    _ = shutdown.notified() => {
                        tracing::info!("Shutdown signal received, stopping task for {:?}", key);
                        ln.cancel_wait_invoice();
                        break;
                    }
                    result = ln.wait_any_incoming_payment() => {
                        match result {
                            Ok(mut stream) => {
                                while let Some(request_lookup_id) = stream.next().await {
                                    if let Err(err) = mint.pay_mint_quote_for_request_id(&request_lookup_id).await {
                                        tracing::warn!("{:?}", err);
                                    }
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Could not get incoming payment stream for {:?}: {}",key, err);

                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            }
                        }
                    }
                    }
            }
        });
            }
        }

        // Spawn a task to manage the JoinSet
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(_) => tracing::info!("A task completed successfully."),
                Err(err) => tracing::warn!("A task failed: {:?}", err),
            }
        }

        Ok(())
    }

    /// Fee required for proof set
    #[instrument(skip_all)]
    pub async fn get_proofs_fee(&self, proofs: &Proofs) -> Result<Amount, Error> {
        let mut proofs_per_keyset = HashMap::new();
        let mut fee_per_keyset = HashMap::new();

        for proof in proofs {
            if let std::collections::hash_map::Entry::Vacant(e) =
                fee_per_keyset.entry(proof.keyset_id)
            {
                let mint_keyset_info = self
                    .get_keyset_info(&proof.keyset_id)
                    .await?
                    .ok_or(Error::UnknownKeySet)?;
                e.insert(mint_keyset_info.input_fee_ppk);
            }

            proofs_per_keyset
                .entry(proof.keyset_id)
                .and_modify(|count| *count += 1)
                .or_insert(1);
        }

        let fee = calculate_fee(&proofs_per_keyset, &fee_per_keyset)?;

        Ok(fee)
    }

    /// Get active keysets
    pub async fn get_active_keysets(&self) -> Result<HashMap<CurrencyUnit, Id>, Error> {
        Ok(self
            .signatory
            .keysets()
            .await?
            .into_iter()
            .filter_map(|keyset| {
                if keyset.info.active {
                    Some((keyset.info.unit.clone(), keyset.info.id))
                } else {
                    None
                }
            })
            .collect())
    }

    /// Get keyset info
    pub async fn get_keyset_info(&self, id: &Id) -> Result<Option<MintKeySetInfo>, Error> {
        Ok(self
            .signatory
            .keysets()
            .await?
            .into_iter()
            .filter_map(|keyset| {
                if keyset.info.id == *id {
                    Some(keyset.info)
                } else {
                    None
                }
            })
            .next())
    }

    /// Blind Sign
    #[instrument(skip_all)]
    pub async fn blind_sign(
        &self,
        blinded_message: &BlindedMessage,
    ) -> Result<BlindSignature, Error> {
        self.signatory.blind_sign(blinded_message.to_owned()).await
    }

    /// Verify [`Proof`] meets conditions and is signed
    #[instrument(skip_all)]
    pub async fn verify_proof(&self, proof: &Proof) -> Result<(), Error> {
        self.signatory.verify_proof(proof.to_owned()).await
    }

    /// Verify melt request is valid
    /// Check to see if there is a corresponding mint quote for a melt.
    /// In this case the mint can settle the payment internally and no ln payment is
    /// needed
    #[instrument(skip_all)]
    pub async fn handle_internal_melt_mint(
        &self,
        melt_quote: &MeltQuote,
        melt_request: &MeltRequest<Uuid>,
    ) -> Result<Option<Amount>, Error> {
        let mint_quote = match self
            .localstore
            .get_mint_quote_by_request(&melt_quote.request)
            .await
        {
            Ok(Some(mint_quote)) => mint_quote,
            // Not an internal melt -> mint
            Ok(None) => return Ok(None),
            Err(err) => {
                tracing::debug!("Error attempting to get mint quote: {}", err);
                return Err(Error::Internal);
            }
        };

        // Mint quote has already been settled, proofs should not be burned or held.
        if mint_quote.state == MintQuoteState::Issued || mint_quote.state == MintQuoteState::Paid {
            return Err(Error::RequestAlreadyPaid);
        }

        let inputs_amount_quote_unit = melt_request.proofs_amount().map_err(|_| {
            tracing::error!("Proof inputs in melt quote overflowed");
            Error::AmountOverflow
        })?;

        let mut mint_quote = mint_quote;

        if mint_quote.amount > inputs_amount_quote_unit {
            tracing::debug!(
                "Not enough inuts provided: {} needed {}",
                inputs_amount_quote_unit,
                mint_quote.amount
            );
            return Err(Error::InsufficientFunds);
        }

        mint_quote.state = MintQuoteState::Paid;

        let amount = melt_quote.amount;

        self.update_mint_quote(mint_quote).await?;

        Ok(Some(amount))
    }

    /// Restore
    #[instrument(skip_all)]
    pub async fn restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        let output_len = request.outputs.len();

        let mut outputs = Vec::with_capacity(output_len);
        let mut signatures = Vec::with_capacity(output_len);

        let blinded_message: Vec<PublicKey> =
            request.outputs.iter().map(|b| b.blinded_secret).collect();

        let blinded_signatures = self
            .localstore
            .get_blind_signatures(&blinded_message)
            .await?;

        assert_eq!(blinded_signatures.len(), output_len);

        for (blinded_message, blinded_signature) in
            request.outputs.into_iter().zip(blinded_signatures)
        {
            if let Some(blinded_signature) = blinded_signature {
                outputs.push(blinded_message);
                signatures.push(blinded_signature);
            }
        }

        Ok(RestoreResponse {
            outputs,
            signatures: signatures.clone(),
            promises: Some(signatures),
        })
    }

    /// Get the total amount issed by keyset
    #[instrument(skip_all)]
    pub async fn total_issued(&self) -> Result<HashMap<Id, Amount>, Error> {
        let keysets = self.keysets().await?.keysets;

        let mut total_issued = HashMap::new();

        for keyset in keysets {
            let blinded = self
                .localstore
                .get_blind_signatures_for_keyset(&keyset.id)
                .await?;

            let total = Amount::try_sum(blinded.iter().map(|b| b.amount))?;

            total_issued.insert(keyset.id, total);
        }

        Ok(total_issued)
    }

    /// Total redeemed for keyset
    #[instrument(skip_all)]
    pub async fn total_redeemed(&self) -> Result<HashMap<Id, Amount>, Error> {
        let keysets = self.keysets().await?.keysets;

        let mut total_redeemed = HashMap::new();

        for keyset in keysets {
            let (proofs, state) = self.localstore.get_proofs_by_keyset_id(&keyset.id).await?;

            let total_spent =
                Amount::try_sum(proofs.iter().zip(state).filter_map(|(p, s)| {
                    match s == Some(State::Spent) {
                        true => Some(p.amount),
                        false => None,
                    }
                }))?;

            total_redeemed.insert(keyset.id, total_spent);
        }

        Ok(total_redeemed)
    }
}

/// Generate new [`MintKeySetInfo`] from path
#[instrument(skip_all)]
fn create_new_keyset<C: secp256k1::Signing>(
    secp: &secp256k1::Secp256k1<C>,
    xpriv: Xpriv,
    derivation_path: DerivationPath,
    derivation_path_index: Option<u32>,
    unit: CurrencyUnit,
    max_order: u8,
    input_fee_ppk: u64,
) -> (MintKeySet, MintKeySetInfo) {
    let keyset = MintKeySet::generate(
        secp,
        xpriv
            .derive_priv(secp, &derivation_path)
            .expect("RNG busted"),
        unit,
        max_order,
    );
    let keyset_info = MintKeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active: true,
        valid_from: unix_time(),
        valid_to: None,
        derivation_path,
        derivation_path_index,
        max_order,
        input_fee_ppk,
    };
    (keyset, keyset_info)
}

#[cfg(test)]
mod tests {
    use cdk_common::common::PaymentProcessorKey;
    use cdk_sqlite::mint::memory::new_with_state;
    use uuid::Uuid;

    use super::*;

    #[derive(Default)]
    struct MintConfig<'a> {
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        seed: &'a [u8],
        mint_info: MintInfo,
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
        melt_requests: Vec<(MeltRequest<Uuid>, PaymentProcessorKey)>,
    }

    async fn create_mint(config: MintConfig<'_>) -> Mint {
        let localstore = Arc::new(
            new_with_state(
                config.active_keysets,
                config.keysets,
                config.mint_quotes,
                config.melt_quotes,
                config.pending_proofs,
                config.spent_proofs,
                config.melt_requests,
                config.mint_info,
            )
            .await
            .unwrap(),
        );

        let signatory = Arc::new(
            cdk_signatory::memory::Memory::new(
                localstore.clone(),
                None,
                config.seed,
                config.supported_units,
                HashMap::new(),
            )
            .await
            .expect("Failed to create signatory"),
        );

        Mint::new(signatory, localstore, HashMap::new())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn mint_mod_new_mint() {
        let config = MintConfig::<'_> {
            ..Default::default()
        };
        let mint = create_mint(config).await;

        assert_eq!(
            mint.pubkeys().await.unwrap(),
            KeysResponse {
                keysets: Vec::new()
            }
        );

        assert_eq!(
            mint.keysets().await.unwrap(),
            KeysetResponse {
                keysets: Vec::new()
            }
        );

        assert_eq!(
            mint.total_issued().await.unwrap(),
            HashMap::<nut02::Id, Amount>::new()
        );

        assert_eq!(
            mint.total_redeemed().await.unwrap(),
            HashMap::<nut02::Id, Amount>::new()
        );
    }

    #[tokio::test]
    async fn mint_mod_rotate_keyset() {
        let config = MintConfig::<'_> {
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let keysets = mint.keysets().await.unwrap();
        assert!(keysets.keysets.is_empty());

        // generate the first keyset and set it to active
        mint.rotate_keyset(CurrencyUnit::default(), 0, 1, 1)
            .await
            .expect("test");

        let keysets = mint.keysets().await.unwrap();
        assert!(keysets.keysets.len().eq(&1));
        assert!(keysets.keysets[0].active);
        let first_keyset_id = keysets.keysets[0].id;

        // set the first keyset to inactive and generate a new keyset
        mint.rotate_keyset(CurrencyUnit::default(), 1, 1, 1)
            .await
            .expect("test");

        let keysets = mint.keysets().await.unwrap();

        assert!(keysets.keysets.len().eq(&2));
        for keyset in &keysets.keysets {
            if keyset.id == first_keyset_id {
                assert!(!keyset.active);
            } else {
                assert!(keyset.active);
            }
        }
    }
}
