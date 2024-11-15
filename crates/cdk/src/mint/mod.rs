//! Cashu Mint

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::secp256k1::{self, Secp256k1};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, RwLock};
use tokio::task::JoinSet;
use tracing::instrument;

use crate::cdk_database::{self, MintDatabase};
use crate::cdk_lightning::{self, MintLightning};
use crate::dhke::{sign_message, verify_message};
use crate::error::Error;
use crate::fees::calculate_fee;
use crate::mint_url::MintUrl;
use crate::nuts::*;
use crate::types::{LnKey, QuoteTTL};
use crate::util::unix_time;
use crate::Amount;

mod builder;
mod check_spendable;
mod info;
mod keysets;
mod melt;
mod mint_nut04;
mod swap;
pub mod types;

pub use builder::{MintBuilder, MintMeltLimits};
pub use types::{MeltQuote, MintQuote};

/// Cashu Mint
#[derive(Clone)]
pub struct Mint {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Mint Info
    pub mint_info: MintInfo,
    /// Quotes ttl
    pub quote_ttl: QuoteTTL,
    /// Mint Storage backend
    pub localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
    /// Ln backends for mint
    pub ln: HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
    /// Subscription manager
    pub pubsub_manager: Arc<PubSubManager>,
    /// Active Mint Keysets
    keysets: Arc<RwLock<HashMap<Id, MintKeySet>>>,
    secp_ctx: Secp256k1<secp256k1::All>,
    xpriv: Xpriv,
}

impl Mint {
    /// Create new [`Mint`]
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        mint_url: &str,
        seed: &[u8],
        mint_info: MintInfo,
        quote_ttl: QuoteTTL,
        localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
        ln: HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
        // Hashmap where the key is the unit and value is (input fee ppk, max_order)
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
        custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<Self, Error> {
        let secp_ctx = Secp256k1::new();
        let xpriv = Xpriv::new_master(bitcoin::Network::Bitcoin, seed).expect("RNG busted");

        let mut active_keysets = HashMap::new();
        let keysets_infos = localstore.get_keyset_infos().await?;

        let mut active_keyset_units = vec![];

        if !keysets_infos.is_empty() {
            tracing::debug!("Setting all saved keysets to inactive");
            for keyset in keysets_infos.clone() {
                // Set all to in active
                let mut keyset = keyset;
                keyset.active = false;
                localstore.add_keyset_info(keyset).await?;
            }

            let keysets_by_unit: HashMap<CurrencyUnit, Vec<MintKeySetInfo>> =
                keysets_infos.iter().fold(HashMap::new(), |mut acc, ks| {
                    acc.entry(ks.unit.clone()).or_default().push(ks.clone());
                    acc
                });

            for (unit, keysets) in keysets_by_unit {
                let mut keysets = keysets;
                keysets.sort_by(|a, b| b.derivation_path_index.cmp(&a.derivation_path_index));

                let highest_index_keyset = keysets
                    .first()
                    .cloned()
                    .expect("unit will not be added to hashmap if empty");

                let keysets: Vec<MintKeySetInfo> = keysets
                    .into_iter()
                    .filter(|ks| ks.derivation_path_index.is_some())
                    .collect();

                if let Some((input_fee_ppk, max_order)) = supported_units.get(&unit) {
                    let derivation_path_index = if keysets.is_empty() {
                        1
                    } else if &highest_index_keyset.input_fee_ppk == input_fee_ppk
                        && &highest_index_keyset.max_order == max_order
                    {
                        let id = highest_index_keyset.id;
                        let keyset = MintKeySet::generate_from_xpriv(
                            &secp_ctx,
                            xpriv,
                            highest_index_keyset.max_order,
                            highest_index_keyset.unit.clone(),
                            highest_index_keyset.derivation_path.clone(),
                        );
                        active_keysets.insert(id, keyset);
                        let mut keyset_info = highest_index_keyset;
                        keyset_info.active = true;
                        localstore.add_keyset_info(keyset_info).await?;
                        localstore.set_active_keyset(unit, id).await?;
                        continue;
                    } else {
                        highest_index_keyset.derivation_path_index.unwrap_or(0) + 1
                    };

                    let derivation_path = match custom_paths.get(&unit) {
                        Some(path) => path.clone(),
                        None => derivation_path_from_unit(unit.clone(), derivation_path_index)
                            .ok_or(Error::UnsupportedUnit)?,
                    };

                    let (keyset, keyset_info) = create_new_keyset(
                        &secp_ctx,
                        xpriv,
                        derivation_path,
                        Some(derivation_path_index),
                        unit.clone(),
                        *max_order,
                        *input_fee_ppk,
                    );

                    let id = keyset_info.id;
                    localstore.add_keyset_info(keyset_info).await?;
                    localstore.set_active_keyset(unit.clone(), id).await?;
                    active_keysets.insert(id, keyset);
                    active_keyset_units.push(unit.clone());
                }
            }
        }

        for (unit, (fee, max_order)) in supported_units {
            if !active_keyset_units.contains(&unit) {
                let derivation_path = match custom_paths.get(&unit) {
                    Some(path) => path.clone(),
                    None => {
                        derivation_path_from_unit(unit.clone(), 0).ok_or(Error::UnsupportedUnit)?
                    }
                };

                let (keyset, keyset_info) = create_new_keyset(
                    &secp_ctx,
                    xpriv,
                    derivation_path,
                    Some(0),
                    unit.clone(),
                    max_order,
                    fee,
                );

                let id = keyset_info.id;
                localstore.add_keyset_info(keyset_info).await?;
                localstore.set_active_keyset(unit, id).await?;
                active_keysets.insert(id, keyset);
            }
        }

        Ok(Self {
            mint_url: MintUrl::from_str(mint_url)?,
            keysets: Arc::new(RwLock::new(active_keysets)),
            pubsub_manager: Arc::new(localstore.clone().into()),
            secp_ctx,
            quote_ttl,
            xpriv,
            localstore,
            mint_info,
            ln,
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
        let BlindedMessage {
            amount,
            blinded_secret,
            keyset_id,
            ..
        } = blinded_message;
        self.ensure_keyset_loaded(keyset_id).await?;

        let keyset_info = self
            .localstore
            .get_keyset_info(keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?;

        let active = self
            .localstore
            .get_active_keyset_id(&keyset_info.unit)
            .await?
            .ok_or(Error::InactiveKeyset)?;

        // Check that the keyset is active and should be used to sign
        if keyset_info.id.ne(&active) {
            return Err(Error::InactiveKeyset);
        }

        let keysets = self.keysets.read().await;
        let keyset = keysets.get(keyset_id).ok_or(Error::UnknownKeySet)?;

        let key_pair = match keyset.keys.get(amount) {
            Some(key_pair) => key_pair,
            None => return Err(Error::AmountKey),
        };

        let c = sign_message(&key_pair.secret_key, blinded_secret)?;

        let blinded_signature = BlindSignature::new(
            *amount,
            c,
            keyset_info.id,
            &blinded_message.blinded_secret,
            key_pair.secret_key.clone(),
        )?;

        Ok(blinded_signature)
    }

    /// Verify [`Proof`] meets conditions and is signed
    #[instrument(skip_all)]
    pub async fn verify_proof(&self, proof: &Proof) -> Result<(), Error> {
        // Check if secret is a nut10 secret with conditions
        if let Ok(secret) =
            <&crate::secret::Secret as TryInto<crate::nuts::nut10::Secret>>::try_into(&proof.secret)
        {
            // Checks and verifes known secret kinds.
            // If it is an unknown secret kind it will be treated as a normal secret.
            // Spending conditions will **not** be check. It is up to the wallet to ensure
            // only supported secret kinds are used as there is no way for the mint to
            // enforce only signing supported secrets as they are blinded at
            // that point.
            match secret.kind {
                Kind::P2PK => {
                    proof.verify_p2pk()?;
                }
                Kind::HTLC => {
                    proof.verify_htlc()?;
                }
            }
        }

        self.ensure_keyset_loaded(&proof.keyset_id).await?;
        let keysets = self.keysets.read().await;
        let keyset = keysets.get(&proof.keyset_id).ok_or(Error::UnknownKeySet)?;

        let keypair = match keyset.keys.get(&proof.amount) {
            Some(key_pair) => key_pair,
            None => return Err(Error::AmountKey),
        };

        verify_message(&keypair.secret_key, proof.c, proof.secret.as_bytes())?;

        Ok(())
    }

    /// Verify melt request is valid
    /// Check to see if there is a corresponding mint quote for a melt.
    /// In this case the mint can settle the payment internally and no ln payment is
    /// needed
    #[instrument(skip_all)]
    pub async fn handle_internal_melt_mint(
        &self,
        melt_quote: &MeltQuote,
        melt_request: &MeltBolt11Request,
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

/// Mint Keyset Info
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintKeySetInfo {
    /// Keyset [`Id`]
    pub id: Id,
    /// Keyset [`CurrencyUnit`]
    pub unit: CurrencyUnit,
    /// Keyset active or inactive
    /// Mint will only issue new [`BlindSignature`] on active keysets
    pub active: bool,
    /// Starting unix time Keyset is valid from
    pub valid_from: u64,
    /// When the Keyset is valid to
    /// This is not shown to the wallet and can only be used internally
    pub valid_to: Option<u64>,
    /// [`DerivationPath`] keyset
    pub derivation_path: DerivationPath,
    /// DerivationPath index of Keyset
    pub derivation_path_index: Option<u32>,
    /// Max order of keyset
    pub max_order: u8,
    /// Input Fee ppk
    #[serde(default = "default_fee")]
    pub input_fee_ppk: u64,
}

fn default_fee() -> u64 {
    0
}

impl From<MintKeySetInfo> for KeySetInfo {
    fn from(keyset_info: MintKeySetInfo) -> Self {
        Self {
            id: keyset_info.id,
            unit: keyset_info.unit,
            active: keyset_info.active,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }
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

fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> Option<DerivationPath> {
    let unit_index = match unit.derivation_index() {
        Some(index) => index,
        None => return None,
    };

    Some(DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(unit_index).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ]))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use bitcoin::Network;
    use secp256k1::Secp256k1;

    use super::*;
    use crate::types::LnKey;

    #[test]
    fn mint_mod_generate_keyset_from_seed() {
        let seed = "test_seed".as_bytes();
        let keyset = MintKeySet::generate_from_seed(
            &Secp256k1::new(),
            seed,
            2,
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0257aed43bf2c1cdbe3e7ae2db2b27a723c6746fc7415e09748f6847916c09176e",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "03ad95811e51adb6231613f9b54ba2ba31e4442c9db9d69f8df42c2b26fbfed26e",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    #[test]
    fn mint_mod_generate_keyset_from_xpriv() {
        let seed = "test_seed".as_bytes();
        let network = Network::Bitcoin;
        let xpriv = Xpriv::new_master(network, seed).expect("Failed to create xpriv");
        let keyset = MintKeySet::generate_from_xpriv(
            &Secp256k1::new(),
            xpriv,
            2,
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0).unwrap(),
        );

        assert_eq!(keyset.unit, CurrencyUnit::Sat);
        assert_eq!(keyset.keys.len(), 2);

        let expected_amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = vec![
            (
                Amount::from(1),
                PublicKey::from_hex(
                    "0257aed43bf2c1cdbe3e7ae2db2b27a723c6746fc7415e09748f6847916c09176e",
                )
                .unwrap(),
            ),
            (
                Amount::from(2),
                PublicKey::from_hex(
                    "03ad95811e51adb6231613f9b54ba2ba31e4442c9db9d69f8df42c2b26fbfed26e",
                )
                .unwrap(),
            ),
        ]
        .into_iter()
        .collect();

        let amounts_and_pubkeys: HashSet<(Amount, PublicKey)> = keyset
            .keys
            .iter()
            .map(|(amount, pair)| (*amount, pair.public_key))
            .collect();

        assert_eq!(amounts_and_pubkeys, expected_amounts_and_pubkeys);
    }

    use cdk_database::mint_memory::MintMemoryDatabase;

    #[derive(Default)]
    struct MintConfig<'a> {
        active_keysets: HashMap<CurrencyUnit, Id>,
        keysets: Vec<MintKeySetInfo>,
        mint_quotes: Vec<MintQuote>,
        melt_quotes: Vec<MeltQuote>,
        pending_proofs: Proofs,
        spent_proofs: Proofs,
        blinded_signatures: HashMap<[u8; 33], BlindSignature>,
        quote_proofs: HashMap<String, Vec<PublicKey>>,
        quote_signatures: HashMap<String, Vec<BlindSignature>>,
        mint_url: &'a str,
        seed: &'a [u8],
        mint_info: MintInfo,
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
        melt_requests: Vec<(MeltBolt11Request, LnKey)>,
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

        Mint::new(
            config.mint_url,
            config.seed,
            config.mint_info,
            config.quote_ttl,
            localstore,
            HashMap::new(),
            config.supported_units,
            HashMap::new(),
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

        let keys = mint.keysets.read().await;

        let expected_keys = r#"{"005f6e8c540c9e61":{"id":"005f6e8c540c9e61","unit":"sat","keys":{"1":{"public_key":"03e8aded7525acee36e3394e28f2dcbc012533ef2a2b085a55fc291d311afee3ef","secret_key":"32ee9fc0723772aed4c7b8ac0a02ffe390e54a4e0b037ec6035c2afa10ebd873"},"2":{"public_key":"02628c0919e5cb8ce9aed1f81ce313f40e1ab0b33439d5be2abc69d9bb574902e0","secret_key":"48384bf901bbe8f937d601001d067e73b28b435819c009589350c664f9ba872c"},"4":{"public_key":"039e7c7f274e1e8a90c61669e961c944944e6154c0794fccf8084af90252d2848f","secret_key":"1f039c1e54e9e65faae8ecf69492f810b4bb2292beb3734059f2bb4d564786d0"},"8":{"public_key":"02ca0e563ae941700aefcb16a7fb820afbb3258ae924ab520210cb730227a76ca3","secret_key":"ea3c2641d847c9b15c5f32c150b5c9c04d0666af0549e54f51f941cf584442be"},"16":{"public_key":"031dbab0e4f7fb4fb0030f0e1a1dc80668eadd0b1046df3337bb13a7b9c982d392","secret_key":"5b244f8552077e68b30b534e85bd0e8e29ae0108ff47f5cd92522aa524d3288f"},"32":{"public_key":"037241f7ad421374eb764a48e7769b5e2473582316844fda000d6eef28eea8ffb8","secret_key":"95608f61dd690aef34e6a2d4cbef3ad8fddb4537a14480a17512778058e4f5bd"},"64":{"public_key":"02bc9767b4abf88becdac47a59e67ee9a9a80b9864ef57d16084575273ac63c0e7","secret_key":"2e9cd067fafa342f3118bc1e62fbb8e53acdb0f96d51ce8a1e1037e43fad0dce"},"128":{"public_key":"0351e33a076f415c2cadc945bc9bcb75bf4a774b28df8a0605dea1557e5897fed8","secret_key":"7014f27be5e2b77e4951a81c18ae3585d0b037899d8a37b774970427b13d8f65"},"256":{"public_key":"0314b9f4300367c7e64fa85770da90839d2fc2f57d63660f08bb3ebbf90ed76840","secret_key":"1a545bd9c40fc6cf2ab281710e279967e9f4b86cd07761c741da94bc8042c8fb"},"512":{"public_key":"030d95abc7e881d173f4207a3349f4ee442b9e51cc461602d3eb9665b9237e8db3","secret_key":"622984ef16d1cb28e9adc7a7cfea1808d85b4bdabd015977f0320c9f573858b4"},"1024":{"public_key":"0351a68a667c5fc21d66c187baecefa1d65529d06b7ae13112d432b6bca16b0e8c","secret_key":"6a8badfa26129499b60edb96cda4cbcf08f8007589eb558a9d0307bdc56e0ff6"},"2048":{"public_key":"0376166d8dcf97d8b0e9f11867ff0dafd439c90255b36a25be01e37e14741b9c6a","secret_key":"48fe41181636716ce202b3a3303c2475e6d511991930868d907441e1bcbf8566"},"4096":{"public_key":"03d40f47b4e5c4d72f2a977fab5c66b54d945b2836eb888049b1dd9334d1d70304","secret_key":"66a25bf144a3b40c015dd1f630aa4ba81d2242f5aee845e4f378246777b21676"},"8192":{"public_key":"03be18afaf35a29d7bcd5dfd1936d82c1c14691a63f8aa6ece258e16b0c043049b","secret_key":"4ddac662e82f6028888c11bdefd07229d7c1b56987395f106cc9ea5b301695f6"},"16384":{"public_key":"028e9c6ce70f34cd29aad48656bf8345bb5ba2cb4f31fdd978686c37c93f0ab411","secret_key":"83676bd7d047655476baecad2864519f0ffd8e60f779956d2faebcc727caa7bd"},"32768":{"public_key":"0253e34bab4eec93e235c33994e01bf851d5caca4559f07d37b5a5c266de7cf840","secret_key":"d5be522906223f5d92975e2a77f7e166aa121bf93d5fe442d6d132bf67166b04"},"65536":{"public_key":"02684ede207f9ace309b796b5259fc81ef0d4492b4fb5d66cf866b0b4a6f27bec9","secret_key":"20d859b7052d768e007bf285ee11dc0b98a4abfe272a551852b0cce9fb6d5ad4"},"131072":{"public_key":"027cdf7be8b20a49ac7f2f065f7c53764c8926799877858c6b00b888a8aa6741a5","secret_key":"f6eef28183344b32fc0a1fba00cd6cf967614e51d1c990f0bfce8f67c6d9746a"},"262144":{"public_key":"026939b8f766c3ebaf26408e7e54fc833805563e2ef14c8ee4d0435808b005ec4c","secret_key":"690f23e4eaa250c652afeac24d4efb583095a66abf6b87a7f3d17b1f42c5f896"},"524288":{"public_key":"03772542057493a46eed6513b40386e766eedada16560ffde2f776b65794e9f004","secret_key":"fe36e61bea74665f8796b4b62f9501ae6e0d5b16733d2c05c146cd39f89475a0"},"1048576":{"public_key":"02b016346e5a322d371c6e6164b28b31b4d93a51572351ca2f26cdc12e916d9ac3","secret_key":"b9269779e057ce715964caa6d6b5b65672f255e86746e994b6b8c4780cb9d728"},"2097152":{"public_key":"028f25283e36a11df7713934a5287267381f8304aca3c1eb1b89fddce973ef1436","secret_key":"41aec998b9624ddcff97eb7341daa6385b2a8714ed3f12969ef39649f4d641ab"},"4194304":{"public_key":"03e5841d310819a49ec42dfb24839c61f68bbfc93ac68f6dad37fd5b2d204cc535","secret_key":"e5aef2509c56236f004e2df4343beab6406816fb187c3532d4340a9674857c64"},"8388608":{"public_key":"0307ebfeb87b7bca9baa03fad00499e5cc999fa5179ef0b7ad4f555568bcb946f5","secret_key":"369e8dcabcc69a2eabb7363beb66178cafc29e53b02c46cd15374028c3110541"},"16777216":{"public_key":"02f2508e7df981c32f7b0008a273e2a1f19c23bb60a1561dba6b2a95ed1251eb90","secret_key":"f93965b96ed5428bcacd684eff2f43a9777d03adfde867fa0c6efb39c46a7550"},"33554432":{"public_key":"0381883a1517f8c9979a84fcd5f18437b1a2b0020376ecdd2e515dc8d5a157a318","secret_key":"7f5e77c7ed04dff952a7c15564ab551c769243eb65423adfebf46bf54360cd64"},"67108864":{"public_key":"02aa648d39c9a725ef5927db15af6895f0d43c17f0a31faff4406314fc80180086","secret_key":"d34eda86679bf872dfb6faa6449285741bba6c6d582cd9fe5a9152d5752596cc"},"134217728":{"public_key":"0380658e5163fcf274e1ace6c696d1feef4c6068e0d03083d676dc5ef21804f22d","secret_key":"3ad22e92d497309c5b08b2dc01cb5180de3e00d3d703229914906bc847183987"},"268435456":{"public_key":"031526f03de945c638acccb879de837ac3fabff8590057cfb8552ebcf51215f3aa","secret_key":"3a740771e29119b171ab8e79e97499771439e0ab6a082ec96e43baf06a546372"},"536870912":{"public_key":"035eb3e7262e126c5503e1b402db05f87de6556773ae709cb7aa1c3b0986b87566","secret_key":"9b77ee8cd879128c0ea6952dd188e63617fbaa9e66a3bca0244bcceb9b1f7f48"},"1073741824":{"public_key":"03f12e6a0903ed0db87485a296b1dca9d953a8a6919ff88732238fbc672d6bd125","secret_key":"f3947bca4df0f024eade569c81c5c53e167476e074eb81fa6b289e5e10dd4e42"},"2147483648":{"public_key":"02cece3fb38a54581e0646db4b29242b6d78e49313dda46764094f9d128c1059c1","secret_key":"582d54a894cd41441157849e0d16750e5349bd9310776306e7313b255866950b"}}}}"#;

        assert_eq!(expected_keys, serde_json::to_string(&keys.clone()).unwrap());

        Ok(())
    }
}
