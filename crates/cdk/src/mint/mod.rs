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

mod check_spendable;
mod info;
mod keysets;
mod melt;
mod mint_nut04;
mod swap;
pub mod types;

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
    /// Active Mint Keysets
    keysets: Arc<RwLock<HashMap<Id, MintKeySet>>>,
    secp_ctx: Secp256k1<secp256k1::All>,
    xpriv: Xpriv,
}

impl Mint {
    /// Create new [`Mint`]
    pub async fn new(
        mint_url: &str,
        seed: &[u8],
        mint_info: MintInfo,
        quote_ttl: QuoteTTL,
        localstore: Arc<dyn MintDatabase<Err = cdk_database::Error> + Send + Sync>,
        ln: HashMap<LnKey, Arc<dyn MintLightning<Err = cdk_lightning::Error> + Send + Sync>>,
        // Hashmap where the key is the unit and value is (input fee ppk, max_order)
        supported_units: HashMap<CurrencyUnit, (u64, u8)>,
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
                    acc.entry(ks.unit).or_default().push(ks.clone());
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
                            highest_index_keyset.unit,
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

                    let derivation_path = derivation_path_from_unit(unit, derivation_path_index);

                    let (keyset, keyset_info) = create_new_keyset(
                        &secp_ctx,
                        xpriv,
                        derivation_path,
                        Some(derivation_path_index),
                        unit,
                        *max_order,
                        *input_fee_ppk,
                    );

                    let id = keyset_info.id;
                    localstore.add_keyset_info(keyset_info).await?;
                    localstore.set_active_keyset(unit, id).await?;
                    active_keysets.insert(id, keyset);
                    active_keyset_units.push(unit);
                }
            }
        }

        for (unit, (fee, max_order)) in supported_units {
            if !active_keyset_units.contains(&unit) {
                let derivation_path = derivation_path_from_unit(unit, 0);

                let (keyset, keyset_info) = create_new_keyset(
                    &secp_ctx,
                    xpriv,
                    derivation_path,
                    Some(0),
                    unit,
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
                let key = *key;
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
                
                Kind::DLC => {
                    todo!()
                }

                Kind::SCT => {
                    todo!()
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
            signatures,
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
        unit: keyset.unit,
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

fn derivation_path_from_unit(unit: CurrencyUnit, index: u32) -> DerivationPath {
    DerivationPath::from(vec![
        ChildNumber::from_hardened_idx(0).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(unit.derivation_index()).expect("0 is a valid index"),
        ChildNumber::from_hardened_idx(index).expect("0 is a valid index"),
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use bitcoin::Network;
    use secp256k1::Secp256k1;

    use crate::types::LnKey;

    use super::*;

    #[test]
    fn mint_mod_generate_keyset_from_seed() {
        let seed = "test_seed".as_bytes();
        let keyset = MintKeySet::generate_from_seed(
            &Secp256k1::new(),
            seed,
            2,
            CurrencyUnit::Sat,
            derivation_path_from_unit(CurrencyUnit::Sat, 0),
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
            derivation_path_from_unit(CurrencyUnit::Sat, 0),
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
        mint.rotate_keyset(CurrencyUnit::default(), 0, 1, 1).await?;

        let keysets = mint.keysets().await.unwrap();
        assert!(keysets.keysets.len().eq(&1));
        assert!(keysets.keysets[0].active);
        let first_keyset_id = keysets.keysets[0].id;

        // set the first keyset to inactive and generate a new keyset
        mint.rotate_keyset(CurrencyUnit::default(), 1, 1, 1).await?;

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
    async fn test_over_pay_fee() -> anyhow::Result<()> {
        Ok(())
    }
}
