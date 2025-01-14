//! Cashu Mint

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::common::{LnKey, QuoteTTL};
use cdk_common::database::{self, MintDatabase};
use config::SwappableConfig;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use signatory::SignatoryManager;
use subscription::PubSubManager;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tracing::instrument;
use uuid::Uuid;

use crate::cdk_lightning::{self, MintLightning};
use crate::error::Error;
use crate::fees::calculate_fee;
use crate::mint_url::MintUrl;
use crate::nuts::*;
use crate::Amount;

mod builder;
mod check_spendable;
pub mod config;
mod info;
mod keysets;
mod melt;
mod mint_nut04;
pub mod signatory;
mod start_up_check;
pub mod subscription;
mod swap;

/// re-export types
pub use builder::{MintBuilder, MintMeltLimits};
pub use cdk_common::mint::{MeltQuote, MintQuote};
pub use cdk_signatory::MemorySignatory;

/// Cashu Mint
#[derive(Clone)]
pub struct Mint {
    /// Mint Config
    pub config: SwappableConfig,
    /// Mint Storage backend
    pub localstore: Arc<dyn MintDatabase<Err = database::Error> + Send + Sync>,
    /// Ln backends for mint
    pub ln: HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
    /// Subscription manager
    pub pubsub_manager: Arc<PubSubManager>,
    /// Signatory
    pub signatory: Arc<SignatoryManager>,
}

impl Mint {
    /// Create new [`Mint`]
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        mint_url: &str,
        mint_info: MintInfo,
        quote_ttl: QuoteTTL,
        localstore: Arc<dyn MintDatabase<Err = database::Error> + Send + Sync>,
        ln: HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
        signatory: Arc<SignatoryManager>,
    ) -> Result<Self, Error> {
        Ok(Self {
            config: SwappableConfig::new(MintUrl::from_str(mint_url)?, quote_ttl, mint_info),
            pubsub_manager: Arc::new(localstore.clone().into()),
            localstore,
            ln,
            signatory,
        })
    }

    /// Wait for any invoice to be paid
    /// For each backend starts a task that waits for any invoice to be paid
    /// Once invoice is paid mint quote status is updated
    #[allow(clippy::incompatible_msrv)]
    // Clippy thinks select is not stable but it compiles fine on MSRV (1.63.0)
    pub async fn wait_for_paid_invoices(&self, shutdown: Arc<Notify>) -> Result<(), Error> {
        let mint_arc = Arc::new(self.clone());

        let mut join_set = JoinSet::new();

        for (key, ln) in self.ln.iter() {
            if !ln.is_wait_invoice_active() {
                let mint = Arc::clone(&mint_arc);
                let ln = Arc::clone(ln);
                let shutdown = Arc::clone(&shutdown);
                let key = key.clone();
                join_set.spawn(async move {
            if !ln.is_wait_invoice_active() {
            loop {
                tokio::select! {
                    _ = shutdown.notified() => {
                        tracing::info!("Shutdown signal received, stopping task for {:?}", key);
                        ln.cancel_wait_invoice();
                        break;
                    }
                    result = ln.wait_any_invoice() => {
                        match result {
                            Ok(mut stream) => {
                                while let Some(request_lookup_id) = stream.next().await {
                                    if let Err(err) = mint.pay_mint_quote_for_request_id(&request_lookup_id).await {
                                        tracing::warn!("{:?}", err);
                                    }
                                }
                            }
                            Err(err) => {
                                tracing::warn!("Could not get invoice stream for {:?}: {}",key, err);
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            }
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
                    .localstore
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
        melt_request: &MeltBolt11Request<Uuid>,
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
        let keysets = self.localstore.get_keyset_infos().await?;

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
        let keysets = self.localstore.get_keyset_infos().await?;

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

/// Mint Fee Reserve
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeeReserve {
    /// Absolute expected min fee
    pub min_fee_reserve: Amount,
    /// Percentage expected fee
    pub percent_fee_reserve: f32,
}

#[cfg(test)]
mod tests {

    use cdk_common::common::{LnKey, QuoteTTL};
    use cdk_common::mint::MintKeySetInfo;
    use cdk_signatory::MemorySignatory;
    use uuid::Uuid;

    use super::*;
    use crate::cdk_database::mint_memory::MintMemoryDatabase;

    #[derive(Default)]
    struct MintConfig<'a> {
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        blinded_signatures: HashMap<[u8; 33], BlindSignature>,
        quote_proofs: HashMap<Uuid, Vec<PublicKey>>,
        quote_signatures: HashMap<Uuid, Vec<BlindSignature>>,
        mint_url: &'a str,
        seed: &'a [u8],
        mint_info: MintInfo,
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
        melt_requests: Vec<(MeltBolt11Request<Uuid>, LnKey)>,
        quote_ttl: QuoteTTL,
    }

    async fn create_mint(config: MintConfig<'_>) -> Result<Mint, Error> {
        let localstore = Arc::new(
            MintMemoryDatabase::new(
                config.active_keysets,
                config.keysets,
                config.mint_quotes,
                config.melt_quotes,
                config.pending_proofs,
                config.spent_proofs,
                config.quote_proofs,
                config.blinded_signatures,
                config.quote_signatures,
                config.melt_requests,
            )
            .unwrap(),
        );

        let signatory_manager = Arc::new(SignatoryManager::new(Arc::new(
            MemorySignatory::new(
                localstore.clone(),
                config.seed,
                config.supported_units,
                HashMap::new(),
            )
            .await
            .expect("valid signatory"),
        )));

        Mint::new(
            config.mint_url,
            config.mint_info,
            config.quote_ttl,
            localstore.clone(),
            HashMap::new(),
            signatory_manager,
        )
        .await
    }

    #[tokio::test]
    async fn mint_mod_new_mint() -> Result<(), Error> {
        let config = MintConfig::<'_> {
            mint_url: "http://example.com",
            ..Default::default()
        };
        let mint = create_mint(config).await?;

        assert_eq!(mint.get_mint_url().to_string(), "http://example.com");
        let info = mint.mint_info();
        assert!(info.name.is_none());
        assert!(info.pubkey.is_none());
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

        Ok(())
    }

    #[tokio::test]
    async fn mint_mod_rotate_keyset() -> Result<(), Error> {
        let config = MintConfig::<'_> {
            mint_url: "http://example.com",
            ..Default::default()
        };
        let mint = create_mint(config).await?;

        let keysets = mint.keysets().await.unwrap();
        assert!(keysets.keysets.is_empty());

        // generate the first keyset and set it to active
        mint.rotate_keyset(CurrencyUnit::default(), 0, 1, 1, HashMap::new())
            .await?;

        let keysets = mint.keysets().await.unwrap();
        assert!(keysets.keysets.len().eq(&1));
        assert!(keysets.keysets[0].active);
        let first_keyset_id = keysets.keysets[0].id;

        // set the first keyset to inactive and generate a new keyset
        mint.rotate_keyset(CurrencyUnit::default(), 1, 1, 1, HashMap::new())
            .await?;

        let keysets = mint.keysets().await.unwrap();

        assert!(keysets.keysets.len().eq(&2));
        for keyset in &keysets.keysets {
            if keyset.id == first_keyset_id {
                assert!(!keyset.active);
            } else {
                assert!(keyset.active);
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_mint_keyset_gen() -> Result<(), Error> {
        let seed = bip39::Mnemonic::from_str(
            "dismiss price public alone audit gallery ignore process swap dance crane furnace",
        )
        .unwrap();

        println!("{}", seed);

        let config = MintConfig::<'_> {
            mint_url: "http://example.com",
            seed: &seed.to_seed_normalized(""),
            ..Default::default()
        };
        let mint = create_mint(config).await?;

        mint.rotate_keyset(CurrencyUnit::default(), 0, 32, 1, HashMap::new())
            .await?;

        let keys = mint
            .signatory
            .keyset_pubkeys("005f6e8c540c9e61".parse().expect("valid key"))
            .await
            .expect("keys");

        let expected_keys = r#"{"keysets":[{"id":"005f6e8c540c9e61","unit":"sat","keys":{"1":"03e8aded7525acee36e3394e28f2dcbc012533ef2a2b085a55fc291d311afee3ef","2":"02628c0919e5cb8ce9aed1f81ce313f40e1ab0b33439d5be2abc69d9bb574902e0","4":"039e7c7f274e1e8a90c61669e961c944944e6154c0794fccf8084af90252d2848f","8":"02ca0e563ae941700aefcb16a7fb820afbb3258ae924ab520210cb730227a76ca3","16":"031dbab0e4f7fb4fb0030f0e1a1dc80668eadd0b1046df3337bb13a7b9c982d392","32":"037241f7ad421374eb764a48e7769b5e2473582316844fda000d6eef28eea8ffb8","64":"02bc9767b4abf88becdac47a59e67ee9a9a80b9864ef57d16084575273ac63c0e7","128":"0351e33a076f415c2cadc945bc9bcb75bf4a774b28df8a0605dea1557e5897fed8","256":"0314b9f4300367c7e64fa85770da90839d2fc2f57d63660f08bb3ebbf90ed76840","512":"030d95abc7e881d173f4207a3349f4ee442b9e51cc461602d3eb9665b9237e8db3","1024":"0351a68a667c5fc21d66c187baecefa1d65529d06b7ae13112d432b6bca16b0e8c","2048":"0376166d8dcf97d8b0e9f11867ff0dafd439c90255b36a25be01e37e14741b9c6a","4096":"03d40f47b4e5c4d72f2a977fab5c66b54d945b2836eb888049b1dd9334d1d70304","8192":"03be18afaf35a29d7bcd5dfd1936d82c1c14691a63f8aa6ece258e16b0c043049b","16384":"028e9c6ce70f34cd29aad48656bf8345bb5ba2cb4f31fdd978686c37c93f0ab411","32768":"0253e34bab4eec93e235c33994e01bf851d5caca4559f07d37b5a5c266de7cf840","65536":"02684ede207f9ace309b796b5259fc81ef0d4492b4fb5d66cf866b0b4a6f27bec9","131072":"027cdf7be8b20a49ac7f2f065f7c53764c8926799877858c6b00b888a8aa6741a5","262144":"026939b8f766c3ebaf26408e7e54fc833805563e2ef14c8ee4d0435808b005ec4c","524288":"03772542057493a46eed6513b40386e766eedada16560ffde2f776b65794e9f004","1048576":"02b016346e5a322d371c6e6164b28b31b4d93a51572351ca2f26cdc12e916d9ac3","2097152":"028f25283e36a11df7713934a5287267381f8304aca3c1eb1b89fddce973ef1436","4194304":"03e5841d310819a49ec42dfb24839c61f68bbfc93ac68f6dad37fd5b2d204cc535","8388608":"0307ebfeb87b7bca9baa03fad00499e5cc999fa5179ef0b7ad4f555568bcb946f5","16777216":"02f2508e7df981c32f7b0008a273e2a1f19c23bb60a1561dba6b2a95ed1251eb90","33554432":"0381883a1517f8c9979a84fcd5f18437b1a2b0020376ecdd2e515dc8d5a157a318","67108864":"02aa648d39c9a725ef5927db15af6895f0d43c17f0a31faff4406314fc80180086","134217728":"0380658e5163fcf274e1ace6c696d1feef4c6068e0d03083d676dc5ef21804f22d","268435456":"031526f03de945c638acccb879de837ac3fabff8590057cfb8552ebcf51215f3aa","536870912":"035eb3e7262e126c5503e1b402db05f87de6556773ae709cb7aa1c3b0986b87566","1073741824":"03f12e6a0903ed0db87485a296b1dca9d953a8a6919ff88732238fbc672d6bd125","2147483648":"02cece3fb38a54581e0646db4b29242b6d78e49313dda46764094f9d128c1059c1"}}]}"#;

        assert_eq!(expected_keys, serde_json::to_string(&keys.clone()).unwrap());

        mint.rotate_keyset(CurrencyUnit::default(), 1, 32, 2, HashMap::new())
            .await?;

        let keys = mint
            .signatory
            .keyset_pubkeys("00c919b6c4fa90c6".parse().expect("valid key"))
            .await
            .expect("keys");

        assert_ne!(expected_keys, serde_json::to_string(&keys.clone()).unwrap());

        Ok(())
    }
}
