//! Cashu Mint

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use cdk_common::common::{PaymentProcessorKey, QuoteTTL};
#[cfg(feature = "auth")]
use cdk_common::database::MintAuthDatabase;
use cdk_common::database::{self, MintDatabase, MintTransaction};
use cdk_common::nuts::{self, BlindSignature, BlindedMessage, CurrencyUnit, Id, Kind};
use cdk_common::secret;
use cdk_signatory::signatory::{Signatory, SignatoryKeySet};
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
#[cfg(feature = "auth")]
use crate::OidcClient;
use crate::{cdk_database, Amount};

#[cfg(feature = "auth")]
pub(crate) mod auth;
mod builder;
mod check_spendable;
mod issue;
mod keysets;
mod ln;
mod melt;
mod proof_writer;
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
    /// It is implemented in the cdk-signatory crate, and it can be embedded in the mint or it can
    /// be a gRPC client to a remote signatory server.
    signatory: Arc<dyn Signatory + Send + Sync>,
    /// Mint Storage backend
    localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
    /// Auth Storage backend (only available with auth feature)
    #[cfg(feature = "auth")]
    auth_localstore: Option<Arc<dyn MintAuthDatabase<Err = database::Error> + Send + Sync>>,
    /// Payment processors for mint
    payment_processors:
        HashMap<PaymentProcessorKey, Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>>,
    /// Subscription manager
    pubsub_manager: Arc<PubSubManager>,
    #[cfg(feature = "auth")]
    oidc_client: Option<OidcClient>,
    /// In-memory keyset
    keysets: Arc<ArcSwap<Vec<SignatoryKeySet>>>,
}

impl Mint {
    /// Create new [`Mint`] without authentication
    pub async fn new(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
        payment_processors: HashMap<
            PaymentProcessorKey,
            Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
        >,
    ) -> Result<Self, Error> {
        Self::new_internal(
            mint_info,
            signatory,
            localstore,
            #[cfg(feature = "auth")]
            None,
            payment_processors,
        )
        .await
    }

    /// Create new [`Mint`] with authentication support
    #[cfg(feature = "auth")]
    pub async fn new_with_auth(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
        auth_localstore: Arc<dyn MintAuthDatabase<Err = database::Error> + Send + Sync>,
        payment_processors: HashMap<
            PaymentProcessorKey,
            Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
        >,
    ) -> Result<Self, Error> {
        Self::new_internal(
            mint_info,
            signatory,
            localstore,
            Some(auth_localstore),
            payment_processors,
        )
        .await
    }

    /// Internal function to create a new [`Mint`] with shared logic
    #[inline]
    async fn new_internal(
        mint_info: MintInfo,
        signatory: Arc<dyn Signatory + Send + Sync>,
        localstore: Arc<dyn MintDatabase<database::Error> + Send + Sync>,
        #[cfg(feature = "auth")] auth_localstore: Option<
            Arc<dyn database::MintAuthDatabase<Err = database::Error> + Send + Sync>,
        >,
        payment_processors: HashMap<
            PaymentProcessorKey,
            Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
        >,
    ) -> Result<Self, Error> {
        let keysets = signatory.keysets().await?;
        if !keysets
            .keysets
            .iter()
            .any(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
        {
            return Err(Error::NoActiveKeyset);
        }

        tracing::info!(
            "Using Signatory {} with {} active keys",
            signatory.name(),
            keysets
                .keysets
                .iter()
                .filter(|keyset| keyset.active && keyset.unit != CurrencyUnit::Auth)
                .count()
        );

        let mint_store = localstore.clone();
        let mut tx = mint_store.begin_transaction().await?;
        tx.set_mint_info(mint_info.clone()).await?;
        tx.set_quote_ttl(QuoteTTL::default()).await?;
        tx.commit().await?;

        Ok(Self {
            signatory,
            pubsub_manager: Arc::new(localstore.clone().into()),
            localstore,
            #[cfg(feature = "auth")]
            oidc_client: mint_info.nuts.nut21.as_ref().map(|nut21| {
                OidcClient::new(
                    nut21.openid_discovery.clone(),
                    Some(nut21.client_id.clone()),
                )
            }),
            payment_processors,
            #[cfg(feature = "auth")]
            auth_localstore,
            keysets: Arc::new(ArcSwap::new(keysets.keysets.into())),
        })
    }

    /// Get the payment processor for the given unit and payment method
    pub fn get_payment_processor(
        &self,
        unit: CurrencyUnit,
        payment_method: PaymentMethod,
    ) -> Result<Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>, Error> {
        let key = PaymentProcessorKey::new(unit.clone(), payment_method.clone());
        self.payment_processors.get(&key).cloned().ok_or_else(|| {
            tracing::info!(
                "No payment processor set for pair {}, {}",
                unit,
                payment_method
            );
            Error::UnsupportedUnit
        })
    }

    /// Localstore
    pub fn localstore(&self) -> Arc<dyn MintDatabase<database::Error> + Send + Sync> {
        Arc::clone(&self.localstore)
    }

    /// Pub Sub manager
    pub fn pubsub_manager(&self) -> Arc<PubSubManager> {
        Arc::clone(&self.pubsub_manager)
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
        let mut tx = self.localstore.begin_transaction().await?;
        tx.set_mint_info(mint_info).await?;
        Ok(tx.commit().await?)
    }

    /// Get quote ttl
    #[instrument(skip_all)]
    pub async fn quote_ttl(&self) -> Result<QuoteTTL, Error> {
        Ok(self.localstore.get_quote_ttl().await?)
    }

    /// Set quote ttl
    #[instrument(skip_all)]
    pub async fn set_quote_ttl(&self, quote_ttl: QuoteTTL) -> Result<(), Error> {
        let mut tx = self.localstore.begin_transaction().await?;
        tx.set_quote_ttl(quote_ttl).await?;
        Ok(tx.commit().await?)
    }

    /// Wait for any invoice to be paid
    /// For each backend starts a task that waits for any invoice to be paid
    /// Once invoice is paid mint quote status is updated
    #[instrument(skip_all)]
    pub async fn wait_for_paid_invoices(&self, shutdown: Arc<Notify>) -> Result<(), Error> {
        let mint_arc = Arc::new(self.clone());

        let mut join_set = JoinSet::new();

        let mut processor_groups: Vec<(
            Arc<dyn MintPayment<Err = cdk_payment::Error> + Send + Sync>,
            Vec<PaymentProcessorKey>,
        )> = Vec::new();

        for (key, ln) in self.payment_processors.iter() {
            // Check if we already have this processor
            let found = processor_groups.iter_mut().find(|(proc_ref, _)| {
                // Compare Arc pointer equality using ptr_eq
                Arc::ptr_eq(proc_ref, ln)
            });

            if let Some((_, keys)) = found {
                // We found this processor, add the key to its group
                keys.push(key.clone());
            } else {
                // New processor, create a new group
                processor_groups.push((Arc::clone(ln), vec![key.clone()]));
            }
        }

        for (ln, key) in processor_groups {
            if !ln.is_wait_invoice_active() {
                tracing::info!("Wait payment for {:?} inactive starting.", key);
                let mint = Arc::clone(&mint_arc);
                let ln = Arc::clone(&ln);
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
                                    if let Err(err) = mint.pay_mint_quote_for_request_id(request_lookup_id).await {
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
    pub fn get_active_keysets(&self) -> HashMap<CurrencyUnit, Id> {
        self.keysets
            .load()
            .iter()
            .filter_map(|keyset| {
                if keyset.active {
                    Some((keyset.unit.clone(), keyset.id))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get keyset info
    pub fn get_keyset_info(&self, id: &Id) -> Option<MintKeySetInfo> {
        self.keysets
            .load()
            .iter()
            .filter_map(|keyset| {
                if keyset.id == *id {
                    Some(keyset.into())
                } else {
                    None
                }
            })
            .next()
    }

    /// Blind Sign
    #[tracing::instrument(skip_all)]
    pub async fn blind_sign(
        &self,
        blinded_message: BlindedMessage,
    ) -> Result<BlindSignature, Error> {
        self.signatory
            .blind_sign(vec![blinded_message])
            .await?
            .pop()
            .ok_or(Error::Internal)
    }

    /// Verify [`Proof`] meets conditions and is signed
    #[tracing::instrument(skip_all)]
    pub async fn verify_proofs(&self, proofs: Proofs) -> Result<(), Error> {
        proofs
            .iter()
            .map(|proof| {
                // Check if secret is a nut10 secret with conditions
                if let Ok(secret) =
                    <&secret::Secret as TryInto<nuts::nut10::Secret>>::try_into(&proof.secret)
                {
                    // Checks and verifies known secret kinds.
                    // If it is an unknown secret kind it will be treated as a normal secret.
                    // Spending conditions will **not** be check. It is up to the wallet to ensure
                    // only supported secret kinds are used as there is no way for the mint to
                    // enforce only signing supported secrets as they are blinded at
                    // that point.
                    match secret.kind() {
                        Kind::P2PK => {
                            proof.verify_p2pk()?;
                        }
                        Kind::HTLC => {
                            proof.verify_htlc()?;
                        }
                        Kind::Cairo => {
                            proof.verify_cairo()?;
                        }
                    }
                }
                Ok(())
            })
            .collect::<Result<Vec<()>, Error>>()?;

        self.signatory.verify_proofs(proofs).await
    }

    /// Verify melt request is valid
    /// Check to see if there is a corresponding mint quote for a melt.
    /// In this case the mint can settle the payment internally and no ln payment is
    /// needed
    #[instrument(skip_all)]
    pub async fn handle_internal_melt_mint(
        &self,
        tx: &mut Box<dyn MintTransaction<'_, cdk_database::Error> + Send + Sync + '_>,
        melt_quote: &MeltQuote,
        melt_request: &MeltRequest<Uuid>,
    ) -> Result<Option<Amount>, Error> {
        let mint_quote = match tx
            .get_mint_quote_by_request(&melt_quote.request.to_string())
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
        tracing::error!("internal stuff");

        // Mint quote has already been settled, proofs should not be burned or held.
        if mint_quote.state() == MintQuoteState::Issued
            || mint_quote.state() == MintQuoteState::Paid
        {
            return Err(Error::RequestAlreadyPaid);
        }

        let inputs_amount_quote_unit = melt_request.inputs_amount().map_err(|_| {
            tracing::error!("Proof inputs in melt quote overflowed");
            Error::AmountOverflow
        })?;

        if let Some(amount) = mint_quote.amount {
            if amount > inputs_amount_quote_unit {
                tracing::debug!(
                    "Not enough inuts provided: {} needed {}",
                    inputs_amount_quote_unit,
                    amount
                );
                return Err(Error::InsufficientFunds);
            }
        }

        let amount = melt_quote.amount;

        tx.increment_mint_quote_amount_paid(&mint_quote.id, amount, melt_quote.id.to_string())
            .await?;

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
        let keysets = self.keysets().keysets;

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
        let keysets = self.keysets().keysets;

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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use cdk_sqlite::mint::memory::new_with_state;

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
                config.mint_info,
            )
            .await
            .unwrap(),
        );

        let signatory = Arc::new(
            cdk_signatory::db_signatory::DbSignatory::new(
                localstore.clone(),
                config.seed,
                config.supported_units,
                HashMap::new(),
            )
            .await
            .expect("Failed to create signatory"),
        );

        Mint::new(MintInfo::default(), signatory, localstore, HashMap::new())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn mint_mod_new_mint() {
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::default(), (0, 32));
        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        assert_eq!(
            mint.total_issued()
                .await
                .unwrap()
                .into_values()
                .collect::<Vec<_>>(),
            vec![Amount::default()]
        );

        assert_eq!(
            mint.total_issued()
                .await
                .unwrap()
                .into_values()
                .collect::<Vec<_>>(),
            vec![Amount::default()]
        );
    }

    #[tokio::test]
    async fn mint_mod_rotate_keyset() {
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::default(), (0, 32));

        let config = MintConfig::<'_> {
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let keysets = mint.keysets();
        let first_keyset_id = keysets.keysets[0].id;

        // set the first keyset to inactive and generate a new keyset
        mint.rotate_keyset(CurrencyUnit::default(), 1, 1)
            .await
            .expect("test");

        let keysets = mint.keysets();

        assert_eq!(2, keysets.keysets.len());
        for keyset in &keysets.keysets {
            if keyset.id == first_keyset_id {
                assert!(!keyset.active);
            } else {
                assert!(keyset.active);
            }
        }
    }

    #[tokio::test]
    async fn test_mint_keyset_gen() {
        let seed = bip39::Mnemonic::from_str(
            "dismiss price public alone audit gallery ignore process swap dance crane furnace",
        )
        .unwrap();
        let mut supported_units = HashMap::new();
        supported_units.insert(CurrencyUnit::default(), (0, 32));

        let config = MintConfig::<'_> {
            seed: &seed.to_seed_normalized(""),
            supported_units,
            ..Default::default()
        };
        let mint = create_mint(config).await;

        let keys = mint.pubkeys();

        let expected_keys = r#"{"keysets":[{"id":"005f6e8c540c9e61","unit":"sat","keys":{"1":"03e8aded7525acee36e3394e28f2dcbc012533ef2a2b085a55fc291d311afee3ef","1024":"0351a68a667c5fc21d66c187baecefa1d65529d06b7ae13112d432b6bca16b0e8c","1048576":"02b016346e5a322d371c6e6164b28b31b4d93a51572351ca2f26cdc12e916d9ac3","1073741824":"03f12e6a0903ed0db87485a296b1dca9d953a8a6919ff88732238fbc672d6bd125","128":"0351e33a076f415c2cadc945bc9bcb75bf4a774b28df8a0605dea1557e5897fed8","131072":"027cdf7be8b20a49ac7f2f065f7c53764c8926799877858c6b00b888a8aa6741a5","134217728":"0380658e5163fcf274e1ace6c696d1feef4c6068e0d03083d676dc5ef21804f22d","16":"031dbab0e4f7fb4fb0030f0e1a1dc80668eadd0b1046df3337bb13a7b9c982d392","16384":"028e9c6ce70f34cd29aad48656bf8345bb5ba2cb4f31fdd978686c37c93f0ab411","16777216":"02f2508e7df981c32f7b0008a273e2a1f19c23bb60a1561dba6b2a95ed1251eb90","2":"02628c0919e5cb8ce9aed1f81ce313f40e1ab0b33439d5be2abc69d9bb574902e0","2048":"0376166d8dcf97d8b0e9f11867ff0dafd439c90255b36a25be01e37e14741b9c6a","2097152":"028f25283e36a11df7713934a5287267381f8304aca3c1eb1b89fddce973ef1436","2147483648":"02cece3fb38a54581e0646db4b29242b6d78e49313dda46764094f9d128c1059c1","256":"0314b9f4300367c7e64fa85770da90839d2fc2f57d63660f08bb3ebbf90ed76840","262144":"026939b8f766c3ebaf26408e7e54fc833805563e2ef14c8ee4d0435808b005ec4c","268435456":"031526f03de945c638acccb879de837ac3fabff8590057cfb8552ebcf51215f3aa","32":"037241f7ad421374eb764a48e7769b5e2473582316844fda000d6eef28eea8ffb8","32768":"0253e34bab4eec93e235c33994e01bf851d5caca4559f07d37b5a5c266de7cf840","33554432":"0381883a1517f8c9979a84fcd5f18437b1a2b0020376ecdd2e515dc8d5a157a318","4":"039e7c7f274e1e8a90c61669e961c944944e6154c0794fccf8084af90252d2848f","4096":"03d40f47b4e5c4d72f2a977fab5c66b54d945b2836eb888049b1dd9334d1d70304","4194304":"03e5841d310819a49ec42dfb24839c61f68bbfc93ac68f6dad37fd5b2d204cc535","512":"030d95abc7e881d173f4207a3349f4ee442b9e51cc461602d3eb9665b9237e8db3","524288":"03772542057493a46eed6513b40386e766eedada16560ffde2f776b65794e9f004","536870912":"035eb3e7262e126c5503e1b402db05f87de6556773ae709cb7aa1c3b0986b87566","64":"02bc9767b4abf88becdac47a59e67ee9a9a80b9864ef57d16084575273ac63c0e7","65536":"02684ede207f9ace309b796b5259fc81ef0d4492b4fb5d66cf866b0b4a6f27bec9","67108864":"02aa648d39c9a725ef5927db15af6895f0d43c17f0a31faff4406314fc80180086","8":"02ca0e563ae941700aefcb16a7fb820afbb3258ae924ab520210cb730227a76ca3","8192":"03be18afaf35a29d7bcd5dfd1936d82c1c14691a63f8aa6ece258e16b0c043049b","8388608":"0307ebfeb87b7bca9baa03fad00499e5cc999fa5179ef0b7ad4f555568bcb946f5"}}]}"#;

        assert_eq!(expected_keys, serde_json::to_string(&keys.clone()).unwrap());
    }
}
