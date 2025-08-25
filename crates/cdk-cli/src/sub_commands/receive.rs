use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use cairo_lang_runner::Arg;
use cairo_prove::execute::execute;
use cairo_prove::prove::{prove, prover_input_from_runner};
use cashu::CairoWitness;
use cdk::nuts::{SecretKey, Token};
use cdk::util::unix_time;
use cdk::wallet::multi_mint_wallet::MultiMintWallet;
use cdk::wallet::types::WalletKey;
use cdk::wallet::ReceiveOptions;
use cdk::Amount;
use clap::Args;
use nostr_sdk::nips::nip04;
use nostr_sdk::{Filter, Keys, Kind, Timestamp};
use starknet_types_core::felt::Felt;
use stwo_cairo_prover::stwo_prover::core::fri::FriConfig;
use stwo_cairo_prover::stwo_prover::core::pcs::PcsConfig;

use crate::nostr_storage;
use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct ReceiveSubCommand {
    /// Cashu Token
    token: Option<String>,
    /// Signing Key
    #[arg(short, long, action = clap::ArgAction::Append)]
    signing_key: Vec<String>,
    /// Nostr key
    #[arg(short, long)]
    nostr_key: Option<String>,
    /// Nostr relay
    #[arg(short, long, action = clap::ArgAction::Append)]
    relay: Vec<String>,
    /// Unix time to query nostr from
    #[arg(long)]
    since: Option<u64>,
    /// Preimage
    #[arg(short, long,  action = clap::ArgAction::Append)]
    preimage: Vec<String>,
    /// Generate witness from Cairo executable
    /// <cairo_executable> <n_inputs> <input1> <input2> ...
    #[arg(long, action = clap::ArgAction::Append, num_args = 1.., value_terminator = "--")]
    cairo: Vec<String>,
}

fn cairo_prove(executable_path: &Path, args: Vec<String>, with_bootloader: bool) -> CairoWitness {
    let executable = serde_json::from_reader(
        std::fs::File::open(executable_path).expect("Failed to open Cairo executable file"),
    )
    .expect("Failed to parse Cairo executable JSON");

    let args: Vec<Arg> = args
        .iter()
        .map(|a| {
            Felt::from_dec_str(a)
                .expect("Invalid argument for Cairo proof")
                .into()
        })
        .collect();

    let runner = execute(executable, args);
    let prover_input = prover_input_from_runner(&runner);

    let with_pedersen = prover_input.public_segment_context[1]; // pedersen builtin

    let pcs_config = PcsConfig {
        pow_bits: 26,
        fri_config: FriConfig {
            log_last_layer_degree_bound: 0,
            log_blowup_factor: 1,
            n_queries: 70,
        },
    };

    let start = Instant::now();
    let cairo_proof = prove(prover_input, pcs_config);
    println!(
        "[cairo_prove fn] Cairo proof generated successfully in {} ms",
        start.elapsed().as_millis()
    );
    let cairo_proof_json = serde_json::to_string(&cairo_proof).unwrap();

    CairoWitness {
        cairo_proof_json,
        with_pedersen,
        with_bootloader,
    }
}

pub async fn receive(
    multi_mint_wallet: &MultiMintWallet,
    sub_command_args: &ReceiveSubCommand,
    work_dir: &Path,
) -> Result<()> {
    let mut signing_keys = Vec::new();

    if !sub_command_args.signing_key.is_empty() {
        let mut s_keys: Vec<SecretKey> = sub_command_args
            .signing_key
            .iter()
            .map(|s| {
                if s.starts_with("nsec") {
                    let nostr_key = nostr_sdk::SecretKey::from_str(s).expect("Invalid secret key");

                    SecretKey::from_str(&nostr_key.to_secret_hex())
                } else {
                    SecretKey::from_str(s)
                }
            })
            .collect::<Result<Vec<SecretKey>, _>>()?;
        signing_keys.append(&mut s_keys);
    }

    let mut cairo_witnesses: Vec<CairoWitness> = Vec::new();
    if !sub_command_args.cairo.is_empty() {
        let cairo_args = &sub_command_args.cairo;
        if cairo_args.len() < 2 {
            return Err(anyhow!(
                "Cairo arguments must include at least the executable path and number of inputs"
            ));
        }
        let exec_path = Path::new(&cairo_args[0]);
        if !exec_path.exists() {
            return Err(anyhow!(
                "Cairo executable file note found: {}",
                exec_path.display()
            ));
        }
        let n_inputs: usize = cairo_args[1]
            .parse::<usize>()
            .map_err(|_| anyhow!("Invalid number of inputs"))?;
        if cairo_args.len() != 2 + n_inputs {
            return Err(anyhow!(
                "Given number of Cairo input arguments does not match the specified number of inputs"
            ));
        }
        let mut input_args = Vec::new();
        for arg in &cairo_args[2..2 + n_inputs] {
            if Felt::from_dec_str(arg).is_err() {
                return Err(anyhow!("Could not parse program input: {} as a Felt", arg));
            }
            input_args.push(arg.clone());
        }

        cairo_witnesses.push(cairo_prove(exec_path, input_args, false));
    }

    let amount = match &sub_command_args.token {
        Some(token_str) => {
            receive_token(
                multi_mint_wallet,
                token_str,
                &signing_keys,
                &sub_command_args.preimage,
                &cairo_witnesses,
            )
            .await?
        }
        None => {
            //wallet.add_p2pk_signing_key(nostr_signing_key).await;
            let nostr_key = match sub_command_args.nostr_key.as_ref() {
                Some(nostr_key) => {
                    let secret_key = nostr_sdk::SecretKey::from_str(nostr_key)?;
                    let secret_key = SecretKey::from_str(&secret_key.to_secret_hex())?;
                    Some(secret_key)
                }
                None => None,
            };

            let nostr_key =
                nostr_key.ok_or(anyhow!("Nostr key required if token is not provided"))?;

            signing_keys.push(nostr_key.clone());

            let relays = sub_command_args.relay.clone();
            let since =
                nostr_storage::get_nostr_last_checked(work_dir, &nostr_key.public_key()).await?;

            let tokens = nostr_receive(relays, nostr_key.clone(), since).await?;

            // Store the current time as last checked
            nostr_storage::store_nostr_last_checked(
                work_dir,
                &nostr_key.public_key(),
                unix_time() as u32,
            )
            .await?;

            let mut total_amount = Amount::ZERO;
            for token_str in &tokens {
                match receive_token(
                    multi_mint_wallet,
                    token_str,
                    &signing_keys,
                    &sub_command_args.preimage,
                    &cairo_witnesses,
                )
                .await
                {
                    Ok(amount) => {
                        total_amount += amount;
                    }
                    Err(err) => {
                        println!("{err}");
                    }
                }
            }

            total_amount
        }
    };

    println!("Received: {amount}");

    Ok(())
}

async fn receive_token(
    multi_mint_wallet: &MultiMintWallet,
    token_str: &str,
    signing_keys: &[SecretKey],
    preimage: &[String],
    cairo_witnesses: &[CairoWitness],
) -> Result<Amount> {
    let token: Token = Token::from_str(token_str)?;

    let mint_url = token.mint_url()?;
    let unit = token.unit().unwrap_or_default();

    if multi_mint_wallet
        .get_wallet(&WalletKey::new(mint_url.clone(), unit.clone()))
        .await
        .is_none()
    {
        get_or_create_wallet(multi_mint_wallet, &mint_url, unit).await?;
    }

    let amount = multi_mint_wallet
        .receive(
            token_str,
            ReceiveOptions {
                p2pk_signing_keys: signing_keys.to_vec(),
                preimages: preimage.to_vec(),
                cairo_witnesses: cairo_witnesses.to_vec(),
                ..Default::default()
            },
        )
        .await?;
    Ok(amount)
}

/// Receive tokens sent to nostr pubkey via dm
async fn nostr_receive(
    relays: Vec<String>,
    nostr_signing_key: SecretKey,
    since: Option<u32>,
) -> Result<HashSet<String>> {
    let verifying_key = nostr_signing_key.public_key();

    let x_only_pubkey = verifying_key.x_only_public_key();

    let nostr_pubkey = nostr_sdk::PublicKey::from_hex(&x_only_pubkey.to_string())?;

    let since = since.map(|s| Timestamp::from(s as u64));

    let filter = match since {
        Some(since) => Filter::new()
            .pubkey(nostr_pubkey)
            .kind(Kind::EncryptedDirectMessage)
            .since(since),
        None => Filter::new()
            .pubkey(nostr_pubkey)
            .kind(Kind::EncryptedDirectMessage),
    };

    let client = nostr_sdk::Client::default();

    client.connect().await;

    let events = client
        .fetch_events_from(relays, filter, Duration::from_secs(30))
        .await?;

    let mut tokens: HashSet<String> = HashSet::new();

    let keys = Keys::from_str(&(nostr_signing_key).to_secret_hex())?;

    for event in events {
        if event.kind == Kind::EncryptedDirectMessage {
            if let Ok(msg) = nip04::decrypt(keys.secret_key(), &event.pubkey, event.content) {
                if let Some(token) = cdk::wallet::util::token_from_text(&msg) {
                    tokens.insert(token.to_string());
                }
            } else {
                tracing::error!("Impossible to decrypt direct message");
            }
        }
    }

    Ok(tokens)
}
