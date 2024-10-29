#![doc = include_str!("./README.md")]

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use bitcoin::bip32::Xpriv;
use bitcoin::Network;
use tracing::instrument;

use crate::amount::SplitTarget;
use crate::cdk_database::{self, WalletDatabase};
use crate::dhke::construct_proofs;
use crate::error::Error;
use crate::fees::calculate_fee;
use crate::mint_url::MintUrl;
use crate::nuts::nut00::token::Token;
use crate::nuts::nutdlc::{
    ClaimDLCPayout, DLCOutcome, DLCPayout, DLCPayoutWitness, DLCRegistrationResponse,
    DLCSettlement, DLCStatusResponse, PostDLCPayoutRequest, PostDLCRegistrationRequest,
    PostSettleDLCRequest, DLC,
};
use crate::nuts::{
    nut10, BlindedMessage, CurrencyUnit, Id, Keys, MintInfo, MintQuoteState, PreMintSecrets, Proof,
    Proofs, RestoreRequest, SpendingConditions, State,
};
use crate::types::ProofInfo;
use crate::{Amount, HttpClient};

mod balance;
pub mod client;
mod keysets;
mod melt;
mod mint;
pub mod multi_mint_wallet;
mod proofs;
mod receive;
mod send;
mod swap;
pub mod types;
pub mod util;

use crate::nuts::nut00::ProofsMethods;
pub use multi_mint_wallet::MultiMintWallet;
pub use types::{MeltQuote, MintQuote, SendKind};

/// CDK Wallet
///
/// The CDK [`Wallet`] is a high level cashu wallet.
///
/// A [`Wallet`] is for a single mint and single unit.
#[derive(Debug, Clone)]
pub struct Wallet {
    /// Mint Url
    pub mint_url: MintUrl,
    /// Unit
    pub unit: CurrencyUnit,
    /// Storage backend
    pub localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
    /// The targeted amount of proofs to have at each size
    pub target_proof_count: usize,
    xpriv: Xpriv,
    client: HttpClient,
}

impl Wallet {
    /// Create new [`Wallet`]
    /// # Synopsis
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use cdk::cdk_database::WalletMemoryDatabase;
    /// use cdk::nuts::CurrencyUnit;
    /// use cdk::wallet::Wallet;
    /// use rand::Rng;
    ///
    /// let seed = rand::thread_rng().gen::<[u8; 32]>();
    /// let mint_url = "https://testnut.cashu.space";
    /// let unit = CurrencyUnit::Sat;
    ///
    /// let localstore = WalletMemoryDatabase::default();
    /// let wallet = Wallet::new(mint_url, unit, Arc::new(localstore), &seed, None);
    /// ```
    pub fn new(
        mint_url: &str,
        unit: CurrencyUnit,
        localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync>,
        seed: &[u8],
        target_proof_count: Option<usize>,
    ) -> Result<Self, Error> {
        let xpriv = Xpriv::new_master(Network::Bitcoin, seed).expect("Could not create master key");

        Ok(Self {
            mint_url: MintUrl::from_str(mint_url)?,
            unit,
            client: HttpClient::new(),
            localstore,
            xpriv,
            target_proof_count: target_proof_count.unwrap_or(3),
        })
    }

    /// Change HTTP client
    pub fn set_client(&mut self, client: HttpClient) {
        self.client = client;
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
                    .get_keyset_by_id(&proof.keyset_id)
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

    /// Get fee for count of proofs in a keyset
    #[instrument(skip_all)]
    pub async fn get_keyset_count_fee(&self, keyset_id: &Id, count: u64) -> Result<Amount, Error> {
        let input_fee_ppk = self
            .localstore
            .get_keyset_by_id(keyset_id)
            .await?
            .ok_or(Error::UnknownKeySet)?
            .input_fee_ppk;

        let fee = (input_fee_ppk * count + 999) / 1000;

        Ok(Amount::from(fee))
    }

    /// Update Mint information and related entries in the event a mint changes
    /// its URL
    #[instrument(skip(self))]
    pub async fn update_mint_url(&mut self, new_mint_url: MintUrl) -> Result<(), Error> {
        self.mint_url = new_mint_url.clone();
        // Where the mint_url is in the database it must be updated
        self.localstore
            .update_mint_url(self.mint_url.clone(), new_mint_url)
            .await?;

        self.localstore.remove_mint(self.mint_url.clone()).await?;
        Ok(())
    }

    /// Qeury mint for current mint information
    #[instrument(skip(self))]
    pub async fn get_mint_info(&self) -> Result<Option<MintInfo>, Error> {
        let mint_info = match self.client.get_mint_info(self.mint_url.clone()).await {
            Ok(mint_info) => Some(mint_info),
            Err(err) => {
                tracing::warn!("Could not get mint info {}", err);
                None
            }
        };

        self.localstore
            .add_mint(self.mint_url.clone(), mint_info.clone())
            .await?;

        tracing::trace!("Mint info updated for {}", self.mint_url);

        Ok(mint_info)
    }

    /// Get amounts needed to refill proof state
    #[instrument(skip(self))]
    pub async fn amounts_needed_for_state_target(&self) -> Result<Vec<Amount>, Error> {
        let unspent_proofs = self.get_unspent_proofs().await?;

        let amounts_count: HashMap<usize, usize> =
            unspent_proofs
                .iter()
                .fold(HashMap::new(), |mut acc, proof| {
                    let amount = proof.amount;
                    let counter = acc.entry(u64::from(amount) as usize).or_insert(0);
                    *counter += 1;
                    acc
                });

        let all_possible_amounts: Vec<usize> = (0..32).map(|i| 2usize.pow(i as u32)).collect();

        let needed_amounts = all_possible_amounts
            .iter()
            .fold(Vec::new(), |mut acc, amount| {
                let count_needed: usize = self
                    .target_proof_count
                    .saturating_sub(*amounts_count.get(amount).unwrap_or(&0));

                for _i in 0..count_needed {
                    acc.push(Amount::from(*amount as u64));
                }

                acc
            });
        Ok(needed_amounts)
    }

    /// Determine [`SplitTarget`] for amount based on state
    #[instrument(skip(self))]
    async fn determine_split_target_values(
        &self,
        change_amount: Amount,
    ) -> Result<SplitTarget, Error> {
        let mut amounts_needed_refill = self.amounts_needed_for_state_target().await?;

        amounts_needed_refill.sort();

        let mut values = Vec::new();

        for amount in amounts_needed_refill {
            let values_sum = Amount::try_sum(values.clone().into_iter())?;
            if values_sum + amount <= change_amount {
                values.push(amount);
            }
        }

        Ok(SplitTarget::Values(values))
    }

    /// Restore
    #[instrument(skip(self))]
    pub async fn restore(&self) -> Result<Amount, Error> {
        // Check that mint is in store of mints
        if self
            .localstore
            .get_mint(self.mint_url.clone())
            .await?
            .is_none()
        {
            self.get_mint_info().await?;
        }

        let keysets = self.get_mint_keysets().await?;

        let mut restored_value = Amount::ZERO;

        for keyset in keysets {
            let keys = self.get_keyset_keys(keyset.id).await?;
            let mut empty_batch = 0;
            let mut start_counter = 0;

            while empty_batch.lt(&3) {
                let premint_secrets = PreMintSecrets::restore_batch(
                    keyset.id,
                    self.xpriv,
                    start_counter,
                    start_counter + 100,
                )?;

                tracing::debug!(
                    "Attempting to restore counter {}-{} for mint {} keyset {}",
                    start_counter,
                    start_counter + 100,
                    self.mint_url,
                    keyset.id
                );

                let restore_request = RestoreRequest {
                    outputs: premint_secrets.blinded_messages(),
                };

                let response = self
                    .client
                    .post_restore(self.mint_url.clone(), restore_request)
                    .await?;

                if response.signatures.is_empty() {
                    empty_batch += 1;
                    start_counter += 100;
                    continue;
                }

                let premint_secrets: Vec<_> = premint_secrets
                    .secrets
                    .iter()
                    .filter(|p| response.outputs.contains(&p.blinded_message))
                    .collect();

                let premint_secrets: Vec<_> = premint_secrets
                    .iter()
                    .filter(|p| response.outputs.contains(&p.blinded_message))
                    .collect();

                // the response outputs and premint secrets should be the same after filtering
                // blinded messages the mint did not have signatures for
                assert_eq!(response.outputs.len(), premint_secrets.len());

                let proofs = construct_proofs(
                    response.signatures,
                    premint_secrets.iter().map(|p| p.r.clone()).collect(),
                    premint_secrets.iter().map(|p| p.secret.clone()).collect(),
                    &keys,
                )?;

                tracing::debug!("Restored {} proofs", proofs.len());

                self.localstore
                    .increment_keyset_counter(&keyset.id, proofs.len() as u32)
                    .await?;

                let states = self.check_proofs_spent(proofs.clone()).await?;

                let unspent_proofs: Vec<Proof> = proofs
                    .iter()
                    .zip(states)
                    .filter(|(_, state)| !state.state.eq(&State::Spent))
                    .map(|(p, _)| p)
                    .cloned()
                    .collect();

                restored_value += unspent_proofs.total_amount()?;

                let unspent_proofs = unspent_proofs
                    .into_iter()
                    .map(|proof| {
                        ProofInfo::new(proof, self.mint_url.clone(), State::Unspent, keyset.unit)
                    })
                    .collect::<Result<Vec<ProofInfo>, _>>()?;

                self.localstore
                    .update_proofs(unspent_proofs, vec![])
                    .await?;

                empty_batch = 0;
                start_counter += 100;
            }
        }
        Ok(restored_value)
    }

    /// Verify all proofs in token have meet the required spend
    /// Can be used to allow a wallet to accept payments offline while reducing
    /// the risk of claiming back to the limits let by the spending_conditions
    #[instrument(skip(self, token))]
    pub fn verify_token_p2pk(
        &self,
        token: &Token,
        spending_conditions: SpendingConditions,
    ) -> Result<(), Error> {
        let (refund_keys, pubkeys, locktime, num_sigs) = match spending_conditions {
            SpendingConditions::P2PKConditions { data, conditions } => {
                let mut pubkeys = vec![data];

                match conditions {
                    Some(conditions) => {
                        pubkeys.extend(conditions.pubkeys.unwrap_or_default());

                        (
                            conditions.refund_keys,
                            Some(pubkeys),
                            conditions.locktime,
                            conditions.num_sigs,
                        )
                    }
                    None => (None, Some(pubkeys), None, None),
                }
            }
            SpendingConditions::HTLCConditions {
                conditions,
                data: _,
            } => match conditions {
                Some(conditions) => (
                    conditions.refund_keys,
                    conditions.pubkeys,
                    conditions.locktime,
                    conditions.num_sigs,
                ),
                None => (None, None, None, None),
            },
            SpendingConditions::DLCConditions { .. } => {
                todo!()
            }

            SpendingConditions::SCTConditions { .. } => {
                todo!()
            }
        };

        if refund_keys.is_some() && locktime.is_none() {
            tracing::warn!(
                "Invalid spending conditions set: Locktime must be set if refund keys are allowed"
            );
            return Err(Error::InvalidSpendConditions(
                "Must set locktime".to_string(),
            ));
        }
        if token.mint_url()? != self.mint_url {
            return Err(Error::IncorrectWallet(format!(
                "Should be {} not {}",
                self.mint_url,
                token.mint_url()?
            )));
        }

        let proofs = token.proofs();
        for proof in proofs {
            let secret: nut10::Secret = (&proof.secret).try_into()?;

            let proof_conditions: SpendingConditions = secret.try_into()?;

            if num_sigs.ne(&proof_conditions.num_sigs()) {
                tracing::debug!(
                    "Spending condition requires: {:?} sigs proof secret specifies: {:?}",
                    num_sigs,
                    proof_conditions.num_sigs()
                );

                return Err(Error::P2PKConditionsNotMet(
                    "Num sigs did not match spending condition".to_string(),
                ));
            }

            let spending_condition_pubkeys = pubkeys.clone().unwrap_or_default();
            let proof_pubkeys = proof_conditions.pubkeys().unwrap_or_default();

            // Check the Proof has the required pubkeys
            if proof_pubkeys.len().ne(&spending_condition_pubkeys.len())
                || !proof_pubkeys
                    .iter()
                    .all(|pubkey| spending_condition_pubkeys.contains(pubkey))
            {
                tracing::debug!("Proof did not included Publickeys meeting condition");
                tracing::debug!("{:?}", proof_pubkeys);
                tracing::debug!("{:?}", spending_condition_pubkeys);
                return Err(Error::P2PKConditionsNotMet(
                    "Pubkeys in proof not allowed by spending condition".to_string(),
                ));
            }

            // If spending condition refund keys is allowed (Some(Empty Vec))
            // If spending conition refund keys is allowed to restricted set of keys check
            // it is one of them Check that proof locktime is > condition
            // locktime

            if let Some(proof_refund_keys) = proof_conditions.refund_keys() {
                let proof_locktime = proof_conditions
                    .locktime()
                    .ok_or(Error::LocktimeNotProvided)?;

                if let (Some(condition_refund_keys), Some(condition_locktime)) =
                    (&refund_keys, locktime)
                {
                    // Proof locktime must be greater then condition locktime to ensure it
                    // cannot be claimed back
                    if proof_locktime.lt(&condition_locktime) {
                        return Err(Error::P2PKConditionsNotMet(
                            "Proof locktime less then required".to_string(),
                        ));
                    }

                    // A non empty condition refund key list is used as a restricted set of keys
                    // returns are allowed to An empty list means the
                    // proof can be refunded to anykey set in the secret
                    if !condition_refund_keys.is_empty()
                        && !proof_refund_keys
                            .iter()
                            .all(|refund_key| condition_refund_keys.contains(refund_key))
                    {
                        return Err(Error::P2PKConditionsNotMet(
                            "Refund Key not allowed".to_string(),
                        ));
                    }
                } else {
                    // Spending conditions does not allow refund keys
                    return Err(Error::P2PKConditionsNotMet(
                        "Spending condition does not allow refund keys".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    /// Verify all proofs in token have a valid DLEQ proof
    #[instrument(skip(self, token))]
    pub async fn verify_token_dleq(&self, token: &Token) -> Result<(), Error> {
        let mut keys_cache: HashMap<Id, Keys> = HashMap::new();

        // TODO: Get mint url
        // if mint_url != &self.mint_url {
        //     return Err(Error::IncorrectWallet(format!(
        //         "Should be {} not {}",
        //         self.mint_url, mint_url
        //     )));
        // }

        let proofs = token.proofs();
        for proof in proofs {
            let mint_pubkey = match keys_cache.get(&proof.keyset_id) {
                Some(keys) => keys.amount_key(proof.amount),
                None => {
                    let keys = self.get_keyset_keys(proof.keyset_id).await?;

                    let key = keys.amount_key(proof.amount);
                    keys_cache.insert(proof.keyset_id, keys);

                    key
                }
            }
            .ok_or(Error::AmountKey)?;

            proof
                .verify_dleq(mint_pubkey)
                .map_err(|_| Error::CouldNotVerifyDleq)?;
        }

        Ok(())
    }

    /// Register a DLC
    #[instrument(skip(self))]
    pub async fn register_dlc(&self, dlc: DLC) -> Result<(), Error> {
        let fund_dlc_request = PostDLCRegistrationRequest {
            registrations: vec![dlc],
        };

        // TODO: the matching on the response seems to always be `Success` even if there are errors
        let fund_dlc_response = match self
            .client
            .post_register_dlc(self.mint_url.clone(), fund_dlc_request)
            .await?
        {
            DLCRegistrationResponse::Success { funded } => funded,
            DLCRegistrationResponse::Error { errors, .. } => {
                println!("Error registering DLC: {:?}", errors);
                tracing::error!("Error registering DLC: {:?}", errors);
                return Err(Error::Custom("Error registering DLC".to_string()));
            }
        };

        // we are not properly catching the error, so if `funded` is empty, we know the registration failed
        assert!(!fund_dlc_response.is_empty(), "DLC registration failed");

        for funded_dlc in fund_dlc_response {
            let dlc_root = funded_dlc.dlc_root;
            let funding_proof = funded_dlc.funding_proof;
            println!("Funded DLC: {:?}", dlc_root);
            println!("Funding Proof: {:?}", funding_proof);
        }

        Ok(())
    }

    /// Settle DLC
    pub async fn settle_dlc(
        &self,
        dlc_root: &String,
        outcome: DLCOutcome,
        merkle_proof: Vec<[u8; 32]>,
    ) -> Result<(), Error> {
        let merkle_proof_string = merkle_proof
            .iter()
            .map(|p| crate::util::hex::encode(p))
            .collect::<Vec<String>>();
        let settle_dlc_request = PostSettleDLCRequest {
            settlements: vec![DLCSettlement {
                dlc_root: dlc_root.clone(),
                outcome,
                merkle_proof: merkle_proof_string,
            }],
        };

        match self
            .client
            .post_settle_dlc(self.mint_url.clone(), settle_dlc_request)
            .await
        {
            Ok(settle_dlc_response) => {
                println!("Settled DLC: {:?}", settle_dlc_response);
                if settle_dlc_response.settled.is_empty() {
                    tracing::error!("No settled DLCs");
                    return Err(Error::Custom("No settled DLCs".to_string()));
                }
                let mut has_root = false;
                for settled_dlc in settle_dlc_response.settled {
                    if settled_dlc.dlc_root == *dlc_root {
                        has_root = true;
                        break;
                    }
                }
                if !has_root {
                    tracing::error!("No settled DLC with root with root: {:?}", dlc_root);
                    return Err(Error::Custom("No settled DLC with root".to_string()));
                } else {
                    return Ok(());
                }
            }
            Err(err) => {
                println!("Error settling DLC: {:?}", err);
                tracing::error!("Error settling DLC: {:?}", err);
                return Err(Error::Custom("Error settling DLC".to_string()));
            }
        };
    }

    /// Get status of DLC
    pub async fn dlc_status(&self, dlc_root: String) -> Result<DLCStatusResponse, Error> {
        let dlc_status_response = match self.client.status(self.mint_url.clone(), &dlc_root).await {
            Ok(dlc_status_response) => dlc_status_response,
            Err(err) => {
                println!("Error: {:?}", err);
                tracing::error!("Error getting DLC status: {:?}", err);
                return Err(Error::Custom("Error getting DLC status".to_string()));
            }
        };

        Ok(dlc_status_response)
    }

    /// Claim payout for DLC
    pub async fn claim_dlc_payout(
        &self,
        dlc_root: String,
        pubkey: String,
        outputs: Vec<BlindedMessage>,
        signature: Option<String>,
    ) -> Result<DLCPayout, Error> {
        let payout = ClaimDLCPayout {
            dlc_root,
            pubkey,
            outputs,
            witness: DLCPayoutWitness {
                secret: None,
                signature: signature.clone(),
            },
        };

        let payout_request = PostDLCPayoutRequest {
            payouts: vec![payout],
        };

        let payout_response = match self
            .client
            .payout(self.mint_url.clone(), payout_request)
            .await
        {
            Ok(payout_response) => payout_response,
            Err(err) => {
                println!("Error: {:?}", err);
                tracing::error!("Error claiming DLC payout: {:?}", err);
                return Err(Error::Custom("Error claiming DLC payout".to_string()));
            }
        };

        if let Some(errors) = payout_response.errors {
            for error in errors {
                println!("Error: {:?}", error);
            }
            return Err(Error::Custom("Error claiming DLC payout".to_string()));
        }

        Ok(payout_response.paid[0].clone())
    }
}
