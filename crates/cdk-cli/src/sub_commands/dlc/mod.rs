use core::panic;
use std::io::{self, stdin, Write};
use std::str::FromStr;
use std::time::Duration;
use std::vec;

use anyhow::{Error, Result};
use cdk::amount::{Amount, SplitTarget};
use cdk::dhke::construct_proofs;
use cdk::mint_url::MintUrl;
use cdk::nuts::nutdlc::{DLCLeaf, DLCOutcome, DLCRoot, DLCTimeoutLeaf, PayoutStructure};
use cdk::nuts::{self, BlindedMessage, PreMintSecrets, Proofs, State, Token};
use cdk::secret;
use cdk::types::ProofInfo;
use cdk::wallet::multi_mint_wallet::WalletKey;
use cdk::wallet::{MultiMintWallet, Wallet};
use clap::{Args, Subcommand};
use dlc::secp256k1_zkp::hashes::sha256;
use dlc::{
    secp256k1_zkp::{All, Secp256k1},
    OracleInfo,
};
use dlc_messages::oracle_msgs::{EventDescriptor, OracleAnnouncement, OracleAttestation};
use nostr_sdk::{
    hashes::hex::{Case, DisplayHex},
    Client, EventId, Keys, PublicKey, SecretKey,
};
use serde::{Deserialize, Serialize};

use super::balance::mint_balances;

pub mod nostr_events;
pub mod utils;
const RELAYS: [&str; 1] = ["wss://relay.8333.space"];

#[derive(Args)]
pub struct DLCSubCommand {
    #[command(subcommand)]
    pub command: DLCCommands,
}

#[derive(Subcommand)]
pub enum DLCCommands {
    CreateBet {
        key: String,
        oracle_event_id: String,
        counterparty_pubkey: String,
        amount: u64,
        //needs to show user outcomes an let user decide which outcome he wants
    },
    ListOffers {
        key: String,
    },
    DeleteOffers {
        key: String,
    },
    AcceptBet {
        key: String,
        // the event id of the offered bet
        event_id: String,
    },
}

// I imagine this is what will be sent back and forth in the kind 8888 messages
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserBet {
    pub id: i32,
    pub oracle_announcement: OracleAnnouncement,
    oracle_event_id: String,
    alice_outcome: String,
    blinding_factor: String,
    dlc_root: String,
    timeout: u64,
    amount: u64, // TODO: use the Amount type
    locked_ecash: Vec<Token>,
    winning_payout_structure: PayoutStructure,
    winning_counterparty_payout_structure: PayoutStructure,
    timeout_payout_structure: PayoutStructure,
}

/// To manage DLC contracts (ie. creating and accepting bets)
// TODO: Different name?
// TODO: put the wallet in here instead of passing it in every function
pub struct DLC {
    keys: Keys,
    nostr: Client,
    secp: Secp256k1<All>,
}

impl DLC {
    /// Create new [`DLC`]
    pub async fn new(secret_key: &SecretKey) -> Result<Self, Error> {
        let keys = Keys::from_str(&secret_key.display_secret().to_string())?;
        let nostr = Client::new(&keys.clone());
        for relay in RELAYS.iter() {
            nostr.add_relay(relay.to_string()).await?;
        }
        nostr.connect().await;
        let secp: Secp256k1<All> = Secp256k1::gen_new();

        Ok(Self { keys, nostr, secp })
    }

    async fn create_funding_token(
        &self,
        wallet: &Wallet,
        dlc_root: &DLCRoot,
        amount: u64,
    ) -> Result<(Token, secret::Secret), Error> {
        let threshold = 1; // TOOD: this should come from payout structures
        let dlc_conditions =
            nuts::nut11::Conditions::new(None, None, None, None, None, Some(threshold))?;

        let dlc_secret =
            nuts::nut10::Secret::new(nuts::Kind::DLC, dlc_root.to_string(), Some(dlc_conditions));
        // TODO: this will put the same secret into each proof.
        // I'm not sure if the mint will allow us to spend multiple proofs with the same backup secret
        // If not, we can use a p2pk backup, or new backup secret for each proof
        let backup_secret = secret::Secret::generate();

        // NOTE: .try_into() converts Nut10Secret to Secret
        let dlc_secret: secret::Secret = dlc_secret.clone().try_into()?;

        let (sct_conditions, sct_proof) = nuts::nut11::SpendingConditions::new_dlc_sct(
            vec![dlc_secret.clone(), backup_secret.clone()],
            0,
        );

        let available_proofs = wallet.get_unspent_proofs().await?;

        let include_fees = false;

        let selected = wallet
            .select_proofs_to_send(Amount::from(amount), available_proofs, include_fees)
            .await
            .unwrap();

        let mut funding_proofs = wallet
            .swap(
                Some(Amount::from(amount)),
                SplitTarget::default(),
                selected,
                Some(sct_conditions),
                include_fees,
            )
            .await?
            .unwrap();

        for proof in &mut funding_proofs {
            proof.add_sct_witness(dlc_secret.to_string(), sct_proof.clone());
        }

        let token = cdk::nuts::nut00::Token::new(
            MintUrl::from_str("https://testnut.brownduff.rocks").unwrap(),
            funding_proofs.clone(),
            Some(String::from("dlc locking proofs")),
            nuts::CurrencyUnit::Sat,
        );

        Ok((token, backup_secret))
    }

    fn compute_leaves(
        &self,
        announcement: OracleAnnouncement,
        blinding_factor: dlc::secp256k1_zkp::Scalar,
        winning_outcome: &String,
        winning_payout_structure: PayoutStructure,
        winning_counterparty_payout_structure: PayoutStructure,
        timeout_payout_structure: PayoutStructure,
        timeout: u64,
    ) -> Result<(Vec<DLCLeaf>, DLCTimeoutLeaf), Error> {
        let oracle_info = OracleInfo {
            public_key: announcement.oracle_public_key,
            nonces: announcement.oracle_event.oracle_nonces.clone(),
        };

        let all_outcomes = if let EventDescriptor::EnumEvent(ref desc) =
            announcement.oracle_event.event_descriptor
        {
            if !desc.outcomes.contains(&winning_outcome) {
                return Err(Error::msg("Invalid winning outcome"));
            }
            desc.outcomes.clone()
        } else {
            return Err(Error::msg("Digit decomposition event not supported"));
        };

        let leaves: Vec<DLCLeaf> = all_outcomes
            .into_iter()
            .map(|outcome| {
                // hash the outcome
                let msg = vec![
                    dlc::secp256k1_zkp::Message::from_hashed_data::<sha256::Hash>(
                        outcome.as_bytes(),
                    ),
                ];

                // get adaptor point
                let point = dlc::get_adaptor_point_from_oracle_info(
                    &self.secp,
                    &[oracle_info.clone()],
                    &[msg],
                )
                .unwrap();

                // blind adaptor point with Ki_ = Ki + b * G
                let blinded_point = point.add_exp_tweak(&self.secp, &blinding_factor).unwrap();

                let payout = if winning_outcome.contains(&outcome) {
                    // we win
                    winning_payout_structure.clone()
                } else {
                    // they win
                    winning_counterparty_payout_structure.clone()
                };

                DLCLeaf {
                    blinded_locking_point: cdk::nuts::PublicKey::from_slice(
                        &blinded_point.serialize(),
                    )
                    .expect("valid public key"),
                    payout,
                }
            })
            .collect();
        let timeout_leaf = DLCTimeoutLeaf::new(&timeout, &timeout_payout_structure);

        Ok((leaves, timeout_leaf))
    }

    fn signatures_to_secret(
        signatures: &[Vec<dlc::secp256k1_zkp::schnorr::Signature>],
    ) -> Result<dlc::secp256k1_zkp::SecretKey, dlc::Error> {
        let s_values = signatures
            .iter()
            .flatten()
            .map(|x| match dlc::secp_utils::schnorrsig_decompose(x) {
                Ok(v) => Ok(v.1),
                Err(err) => Err(err),
            })
            .collect::<Result<Vec<&[u8]>, dlc::Error>>()?;
        let secret = dlc::secp256k1_zkp::SecretKey::from_slice(s_values[0])?;

        let result = s_values.iter().skip(1).fold(secret, |accum, s| {
            let sec = dlc::secp256k1_zkp::SecretKey::from_slice(s).unwrap();
            accum
                .add_tweak(&dlc::secp256k1_zkp::scalar::Scalar::from(sec))
                .unwrap()
        });

        Ok(result)
    }

    /// Start a new DLC contract, and send to the counterparty
    /// # Arguments
    /// * `announcement` - OracleAnnouncement
    /// * `announcement_id` - Id of kind 88 event
    /// * `counterparty_pubkey` - hex encoded public key of counterparty
    /// * `outcomes` - ??outcomes this user wants to bet on?? I think!
    pub async fn create_bet(
        &self,
        wallet: &Wallet,
        announcement: OracleAnnouncement,
        announcement_id: EventId,
        counterparty_pubkey: nostr_sdk::key::PublicKey,
        outcomes: Vec<String>,
        amount: u64,
    ) -> Result<EventId, Error> {
        let winning_payout_structure = PayoutStructure::default(self.keys.public_key().to_string());
        let winning_counterparty_payout_structure =
            PayoutStructure::default(counterparty_pubkey.to_string());
        // timeout set to 1 hour from event_maturity_epoch
        let timeout = (announcement.oracle_event.event_maturity_epoch as u64)
            + Duration::from_secs(60 * 60).as_secs();
        let timeout_payout_structure = PayoutStructure::default_timeout(vec![
            self.keys.public_key().to_string(),
            counterparty_pubkey.to_string(),
        ]);

        let blinding_factor = dlc::secp256k1_zkp::Scalar::random();
        let winning_outcome = outcomes.first().unwrap().clone();

        let (leaves, timeout_leaf) = self.compute_leaves(
            announcement.clone(),
            blinding_factor,
            &winning_outcome,
            winning_payout_structure.clone(),
            winning_counterparty_payout_structure.clone(),
            timeout_payout_structure.clone(),
            timeout,
        )?;

        let dlc_root = DLCRoot::compute(leaves, Some(timeout_leaf));

        let (token, _backup_secret) = self
            .create_funding_token(&wallet, &dlc_root, amount)
            .await?;

        // TODO: backup the backup secret

        let offer_dlc = UserBet {
            id: 7, // TODO,
            oracle_announcement: announcement.clone(),
            oracle_event_id: announcement_id.to_string(),
            alice_outcome: winning_outcome,
            blinding_factor: blinding_factor.to_be_bytes().to_hex_string(Case::Lower),
            dlc_root: dlc_root.to_string(),
            timeout,
            amount,
            locked_ecash: vec![token],
            winning_payout_structure,
            winning_counterparty_payout_structure,
            timeout_payout_structure,
        };

        let offer_dlc = serde_json::to_string(&offer_dlc)?;

        let offer_dlc_event =
            nostr_events::create_dlc_msg_event(&self.keys, offer_dlc, &counterparty_pubkey)?;

        match self.nostr.send_event(offer_dlc_event).await {
            Ok(event_id) => Ok(event_id.val),
            Err(e) => Err(Error::from(e)),
        }
    }

    pub async fn accept_bet(&self, wallet: &Wallet, bet: &UserBet) -> Result<(), Error> {
        // TODO: validate payout structures
        // TODO: validate dlc_root

        let (funding_token, _backup_secret) = self
            .create_funding_token(wallet, &DLCRoot::from_str(&bet.dlc_root)?, bet.amount)
            .await?;

        // TODO: backup the backup secret

        let counterparty_funding_token = bet.locked_ecash.first().unwrap().clone();

        /* extract proofs from both funding tokens */
        let mut dlc_inputs: Proofs = Vec::new();

        dlc_inputs.extend(funding_token.proofs());
        dlc_inputs.extend(counterparty_funding_token.proofs());

        let dlc_registration = nuts::nutdlc::DLC {
            dlc_root: bet.dlc_root.clone(),
            funding_amount: Amount::from(bet.amount),
            unit: nuts::CurrencyUnit::Sat,
            inputs: dlc_inputs,
        };

        println!("Registering DLC");

        wallet.register_dlc(dlc_registration).await?;

        Ok(())
    }

    pub async fn settle_bet(
        &self,
        wallet: &Wallet,
        bet: &UserBet,
        attestation: OracleAttestation,
    ) -> Result<(), Error> {
        let blinding_factor_bytes: [u8; 32] = nostr_sdk::util::hex::decode(&bet.blinding_factor)
            .map_err(|_| Error::msg("Invalid blinding factor"))?
            .try_into()
            .map_err(|_| Error::msg("Invalid blinding factor length"))?;

        let blinding_factor = dlc::secp256k1_zkp::Scalar::from_be_bytes(blinding_factor_bytes)?;

        assert_eq!(
            bet.blinding_factor,
            blinding_factor.to_be_bytes().to_hex_string(Case::Lower),
            "Blinding factors do not match"
        );

        let (leaves, timeout_leaf) = self.compute_leaves(
            bet.oracle_announcement.clone(),
            blinding_factor,
            &bet.alice_outcome,
            bet.winning_payout_structure.clone(),
            bet.winning_counterparty_payout_structure.clone(),
            bet.timeout_payout_structure.clone(),
            bet.timeout,
        )?;

        let leaf_hashes: Vec<[u8; 32]> = leaves.iter().map(|l| l.hash()).collect();
        let leaf_hashes = vec![leaf_hashes[0], leaf_hashes[1], timeout_leaf.hash()];

        let dlc_root = DLCRoot::compute(leaves.clone(), Some(timeout_leaf));

        assert_eq!(
            bet.dlc_root,
            dlc_root.to_string(),
            "Recomputed dlc_root does not match"
        );

        assert_eq!(attestation.outcomes[0], bet.alice_outcome, "Wrong outcome");

        let merkle_proof = nuts::nutsct::merkle_prove(leaf_hashes, 0);

        let secret = DLC::signatures_to_secret(&[attestation.signatures])?;
        let blinded_secret = secret.add_tweak(&blinding_factor).unwrap();

        let outcome = DLCOutcome {
            blinded_attestation_secret: blinded_secret.display_secret().to_string(),
            payout_structure: leaves[0].payout.clone(),
        };

        wallet
            .settle_dlc(&bet.dlc_root, outcome, merkle_proof)
            .await?;

        Ok(())
    }

    async fn claim_payout(&self, wallet: &Wallet, bet: &UserBet) -> Result<()> {
        let dlc_status = wallet.dlc_status(bet.dlc_root.clone()).await?;

        if !dlc_status.settled {
            return Err(Error::msg("DLC not settled".to_string()));
        }

        let our_debt = if let Some(debts) = dlc_status.debts {
            let our_public_key = self.keys.public_key();
            debts
                .iter()
                .find(|(k, _)| {
                    /* we prefix our public key with "02" to convert our nostr key to 33 bytes */
                    let key_without_prefix = &k[2..];
                    key_without_prefix == our_public_key.to_string()
                })
                .map(|(_, v)| *v)
                .ok_or_else(|| Error::msg("Our public key not found in debts".to_string()))?
        } else {
            return Err(Error::msg("No debts in DLC".to_string()));
        };

        let dlc_root = DLCRoot::from_str(&bet.dlc_root)?.to_bytes();

        let sig = self
            .keys
            .sign_schnorr(&nostr_sdk::secp256k1::Message::from_digest(dlc_root));

        let keyset_id = wallet.get_active_mint_keyset().await?.id;

        let pre_mint_secrets =
            PreMintSecrets::random(keyset_id, Amount::from(our_debt), &SplitTarget::None)?;
        let outputs: Vec<BlindedMessage> = pre_mint_secrets
            .clone()
            .secrets
            .into_iter()
            .map(|s| s.blinded_message)
            .collect();

        let payout = wallet
            .claim_dlc_payout(
                bet.dlc_root.clone(),
                format!("02{}", self.keys.public_key().to_string()),
                outputs.clone(),
                Some(sig.to_string()),
            )
            .await?;

        let keys = wallet.get_keyset_keys(keyset_id).await?;

        let proofs = construct_proofs(
            payout.outputs.iter().map(|p| p.clone()).collect(),
            pre_mint_secrets.rs(),
            pre_mint_secrets.secrets(),
            &keys,
        )?;

        let proofs = proofs
            .into_iter()
            .map(|proof| {
                ProofInfo::new(
                    proof,
                    wallet.mint_url.clone(),
                    State::Unspent,
                    nuts::CurrencyUnit::Sat,
                )
            })
            .collect::<Result<Vec<ProofInfo>, _>>()?;

        let total_claimed = proofs
            .clone()
            .iter()
            .fold(Amount::ZERO, |acc, p| acc + p.proof.amount);
        println!("Claimed {:?}", total_claimed);

        wallet.localstore.update_proofs(proofs, vec![]).await?;
        Ok(())
    }
}

pub async fn dlc(wallets: &MultiMintWallet, sub_command_args: &DLCSubCommand) -> Result<()> {
    //let keys =
    //   Keys::parse("nsec15jldh0htg2qeeqmqd628js8386fu4xwpnuqddacc64gh0ezdum6qaw574p").unwrap();

    let unit = nuts::CurrencyUnit::Sat;

    match &sub_command_args.command {
        DLCCommands::CreateBet {
            key,
            oracle_event_id,
            counterparty_pubkey,
            amount,
        } => {
            let keys = Keys::parse(key).unwrap();
            let oracle_event_id = EventId::from_hex(oracle_event_id).unwrap();
            let counterparty_pubkey = PublicKey::from_hex(counterparty_pubkey).unwrap();

            let dlc = DLC::new(keys.secret_key()).await?;

            let announcement_event =
                match nostr_events::lookup_announcement_event(oracle_event_id, &dlc.nostr).await {
                    Some(Ok(event)) => event,
                    _ => panic!("Oracle announcement event not found"),
                };

            let oracle_announcement =
                utils::oracle_announcement_from_str(&announcement_event.content);

            println!(
                "Oracle announcement event content: {:?}",
                oracle_announcement
            );

            // // TODO: get the outcomes from the oracle announcement???

            let outcomes = match oracle_announcement.oracle_event.event_descriptor {
                EventDescriptor::EnumEvent(ref e) => e.outcomes.clone(),
                EventDescriptor::DigitDecompositionEvent(_) => unreachable!(),
            };

            for (i, outcome) in outcomes.clone().into_iter().enumerate() {
                println!("outcome {i}: {outcome}");
            }

            let mut input_line = String::new();

            println!("please select outcome by number");

            stdin()
                .read_line(&mut input_line)
                .expect("Failed to read line");
            let choice: i32 = input_line.trim().parse().expect("Input not an integer");

            let outcome_choice = vec![outcomes[choice as usize].clone()];

            println!(
                "You chose outcome {:?} to bet {} on",
                outcome_choice, amount
            );

            /* let user pick which wallet to use */
            let mints_amounts = mint_balances(wallets, &unit).await?;

            println!("Enter a mint number to create a DLC offer for");

            let mut user_input = String::new();
            io::stdout().flush().unwrap();
            stdin().read_line(&mut user_input)?;

            let mint_number: usize = user_input.trim().parse()?;

            if mint_number.gt(&(mints_amounts.len() - 1)) {
                crate::bail!("Invalid mint number");
            }

            let mint_url = mints_amounts[mint_number].0.clone();

            let wallet = match wallets
                .get_wallet(&WalletKey::new(mint_url.clone(), unit))
                .await
            {
                Some(wallet) => wallet.clone(),
                None => {
                    // let wallet = Wallet::new(&mint_url.to_string(), unit, localstore, seed, None)?;

                    // multi_mint_wallet.add_wallet(wallet.clone()).await;
                    // wallet
                    todo!()
                }
            };

            let event_id = dlc
                .create_bet(
                    &wallet,
                    oracle_announcement,
                    oracle_event_id,
                    counterparty_pubkey,
                    outcomes,
                    *amount,
                )
                .await?;

            println!("Event {} sent to {}", event_id, counterparty_pubkey);
        }
        DLCCommands::ListOffers { key } => {
            let keys = Keys::parse(key).unwrap();

            let dlc = DLC::new(keys.secret_key()).await?;

            let bets = nostr_events::list_dlc_offers(&keys, &dlc.nostr, None).await;

            println!("{:?}", bets);
        }
        DLCCommands::DeleteOffers { key } => {
            let keys = Keys::parse(key).unwrap();

            let dlc = DLC::new(keys.secret_key()).await?;

            let bets = nostr_events::delete_all_dlc_offers(&keys, &dlc.nostr).await;

            println!("{:?}", bets);
        }
        DLCCommands::AcceptBet { key, event_id } => {
            let keys = Keys::parse(key).unwrap();
            let event_id = EventId::from_hex(event_id).unwrap();

            let dlc = DLC::new(keys.secret_key()).await?;

            let bet = nostr_events::list_dlc_offers(&keys, &dlc.nostr, Some(event_id))
                .await
                .unwrap()
                .first()
                .unwrap()
                .clone();

            /* let user pick which wallet to use */
            let mints_amounts = mint_balances(wallets, &unit).await?;

            println!("Enter a mint number to create a DLC offer for");

            let mut user_input = String::new();
            io::stdout().flush().unwrap();
            stdin().read_line(&mut user_input)?;

            let mint_number: usize = user_input.trim().parse()?;

            if mint_number.gt(&(mints_amounts.len() - 1)) {
                crate::bail!("Invalid mint number");
            }

            // TODO: wallet needs to be from same mint as bet
            let mint_url = mints_amounts[mint_number].0.clone();

            let wallet = match wallets
                .get_wallet(&WalletKey::new(mint_url.clone(), unit))
                .await
            {
                Some(wallet) => wallet.clone(),
                None => {
                    // let wallet = Wallet::new(&mint_url.to_string(), unit, localstore, seed, None)?;

                    // multi_mint_wallet.add_wallet(wallet.clone()).await;
                    // wallet
                    todo!()
                }
            };

            dlc.accept_bet(&wallet, &bet).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, str::FromStr, sync::Arc};

    use bip39::Mnemonic;
    use cdk::{
        cdk_database::{self, WalletDatabase},
        mint_url::MintUrl,
        wallet::Wallet,
    };
    use cdk_sqlite::WalletSqliteDatabase;
    use dlc_messages::oracle_msgs::EventDescriptor;
    use nostr_sdk::{Client, EventId, Keys};
    use rand::Rng;

    use super::*;
    use crate::sub_commands::dlc::{
        nostr_events::{delete_all_dlc_offers, list_dlc_offers},
        utils::oracle_announcement_from_str,
        DLC,
    };

    const DEFAULT_WORK_DIR: &str = ".cdk-cli";
    const MINT_URL: &str = "https://testnut.brownduff.rocks";

    /// helper function to initialize wallets
    async fn initialize_wallets() -> MultiMintWallet {
        let work_dir = {
            let home_dir = home::home_dir().unwrap();
            home_dir.join(DEFAULT_WORK_DIR)
        };
        let localstore: Arc<dyn WalletDatabase<Err = cdk_database::Error> + Send + Sync> = {
            let sql_path = work_dir.join("cdk-cli.sqlite");
            let sql = WalletSqliteDatabase::new(&sql_path).await.unwrap();

            sql.migrate().await;

            Arc::new(sql)
        };

        let seed_path = work_dir.join("seed");

        let mnemonic = match fs::metadata(seed_path.clone()) {
            Ok(_) => {
                let contents = fs::read_to_string(seed_path.clone()).unwrap();
                Mnemonic::from_str(&contents).unwrap()
            }
            Err(_e) => {
                let mut rng = rand::thread_rng();
                let random_bytes: [u8; 32] = rng.gen();

                let mnemnic = Mnemonic::from_entropy(&random_bytes).unwrap();
                tracing::info!("Using randomly generated seed you will not be able to restore");

                mnemnic
            }
        };

        let mut wallets: Vec<Wallet> = Vec::new();

        let mints = localstore.get_mints().await.unwrap();

        for (mint, _) in mints {
            let wallet = Wallet::new(
                &mint.to_string(),
                cdk::nuts::CurrencyUnit::Sat,
                localstore.clone(),
                &mnemonic.to_seed_normalized(""),
                None,
            )
            .unwrap();

            wallets.push(wallet);
        }

        MultiMintWallet::new(wallets)
    }

    #[tokio::test]
    async fn test_full_flow() {
        let multi_mint_wallet = initialize_wallets().await;
        let wallet = multi_mint_wallet
            .get_wallet(&WalletKey::new(
                MintUrl::from_str(MINT_URL).unwrap(),
                cdk::nuts::CurrencyUnit::Sat,
            ))
            .await
            .unwrap();

        let alice_keys = Keys::generate();
        let bob_keys = Keys::generate();

        let alice_dlc = DLC::new(alice_keys.secret_key()).await.unwrap();
        let bob_dlc = DLC::new(bob_keys.secret_key()).await.unwrap();

        let oracle_event_id =
            EventId::from_hex("f6b983be1d9f984d269b66c80421c66a1ad9fcfecbc7d656f4cb7a8098d4d949")
                .unwrap();

        let announcement_event = match nostr_events::lookup_announcement_event(
            oracle_event_id,
            &alice_dlc.nostr,
        )
        .await
        {
            Some(Ok(event)) => event,
            _ => std::panic!("Oracle announcement event not found"),
        };

        let announcement = utils::oracle_announcement_from_str(&announcement_event.content);

        let descriptor = &announcement.oracle_event.event_descriptor;

        let outcomes = match descriptor {
            EventDescriptor::EnumEvent(ref e) => e.outcomes.clone(),
            EventDescriptor::DigitDecompositionEvent(_) => unreachable!(),
        };
        let alice_outcome = &outcomes.clone()[0];

        let amount = 7;
        println!("Alice is creating a bet for {} sats", amount);
        let offer_event_id = alice_dlc
            .create_bet(
                &wallet,
                announcement,
                oracle_event_id.clone(),
                bob_keys.public_key(),
                vec![alice_outcome.clone()],
                amount,
            )
            .await
            .unwrap();

        let bet = nostr_events::list_dlc_offers(&bob_keys, &bob_dlc.nostr, Some(offer_event_id))
            .await
            .unwrap()
            .first()
            .unwrap()
            .clone();

        println!(
            "Bob is accepting the bet and addiing {:?} sats to the contract",
            amount
        );
        bob_dlc.accept_bet(&wallet, &bet).await.unwrap();

        let attestation_event = nostr_events::lookup_attestation_event(
            EventId::from_hex(bet.oracle_event_id.clone()).unwrap(),
            &alice_dlc.nostr,
        )
        .await
        .unwrap()
        .unwrap();

        let attestation = utils::oracle_attestation_from_str(&attestation_event.content);

        println!("Winning outcome is {:?}", attestation.outcomes[0]);

        println!("Alice is settling the bet");

        alice_dlc
            .settle_bet(&wallet, &bet, attestation)
            .await
            .unwrap();

        println!("Alice is claiming payout");

        alice_dlc.claim_payout(&wallet, &bet).await.unwrap();

        nostr_events::delete_all_dlc_offers(&bob_keys, &bob_dlc.nostr).await;
        nostr_events::delete_all_dlc_offers(&alice_keys, &alice_dlc.nostr).await;
    }

    #[tokio::test]
    async fn test_create_and_post_offer() {
        let multi_mint_wallet = initialize_wallets().await;
        let wallet = multi_mint_wallet
            .get_wallet(&WalletKey::new(
                MintUrl::from_str(MINT_URL).unwrap(),
                cdk::nuts::CurrencyUnit::Sat,
            ))
            .await
            .unwrap();
        const ANNOUNCEMENT: &str = "ypyyyX6pdZUM+OovHftxK9StImd8F7nxmr/eTeyR/5koOVVe/EaNw1MAeJm8LKDV1w74Fr+UJ+83bVP3ynNmjwKbtJr9eP5ie2Exmeod7kw4uNsuXcw6tqJF1FXH3fTF/dgiOwAByEOAEd95715DKrSLVdN/7cGtOlSRTQ0/LsW/p3BiVOdlpccA/dgGDAACBDEyMzQENDU2NwR0ZXN0";
        let announcement = oracle_announcement_from_str(ANNOUNCEMENT);
        let announcement_id =
            EventId::from_hex("d30e6c857a900ebefbf7dc3b678ead9215f4345476067e146ded973971286529")
                .unwrap();
        let keys = Keys::generate();
        let counterparty_keys = Keys::generate();

        let dlc = DLC::new(&keys.secret_key()).await.unwrap();

        let descriptor = &announcement.oracle_event.event_descriptor;

        let outcomes = match descriptor {
            EventDescriptor::EnumEvent(ref e) => e.outcomes.clone(),
            EventDescriptor::DigitDecompositionEvent(_) => unreachable!(),
        };
        let outcome1 = &outcomes.clone()[0];

        let amount = 7;
        let _event_id = dlc
            .create_bet(
                &wallet,
                announcement,
                announcement_id,
                counterparty_keys.public_key(),
                vec![outcome1.clone()],
                amount,
            )
            .await
            .unwrap();

        let client = Client::new(&Keys::generate());
        let relay = "wss://relay.8333.space";
        client.add_relay(relay.to_string()).await.unwrap();
        client.connect().await;

        let offers = list_dlc_offers(&counterparty_keys, &client, None) // error line 74:58 in nostr_events.rs
            .await
            .unwrap(); // if event exists should unwrap to event

        println!("{:?}", offers);

        assert!(offers.len() >= 1);

        /* clean up */
        delete_all_dlc_offers(&keys, &client).await;
    }

    #[tokio::test]
    async fn test_dlc_status() {
        let multi_mint_wallet = initialize_wallets().await;
        let wallet = multi_mint_wallet
            .get_wallet(&WalletKey::new(
                MintUrl::from_str(MINT_URL).unwrap(),
                cdk::nuts::CurrencyUnit::Sat,
            ))
            .await
            .unwrap();

        let dlc_root =
            String::from("1a494a3792ef8084fc2d7ad71c5bddfbfacd8a5bd420d98c4d30f2ad15e03006");

        let dlc_status = wallet.dlc_status(dlc_root.clone()).await.unwrap();
        println!("DLC status: {:?}", dlc_status);
        assert!(dlc_status.settled);
    }
}

// ALICE:
// - pub: d71b2434429b0f038ed35e0e3827bca5e65b6d44d1af9344f73b20ff7ffa93dd
// - priv: b9452287c9e4cf53cf935adbc2341931c68c19d8447fe571ccc8dd9b5ed85584
// BOB:
// - pub: b3e6ae1bdfa18106dafe4992b77149a38623662f78f5f60ee436e457f7965ee2
// - priv: 4e111131d31ad92ed5d37ab87d5046efa730f192f9c8f9b59f6c61caad1f8933

// anouncement_ID: d30e6c857a900ebefbf7dc3b678ead9215f4345476067e146ded973971286529
