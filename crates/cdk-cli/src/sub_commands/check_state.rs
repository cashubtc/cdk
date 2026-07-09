use std::str::FromStr;

use anyhow::{bail, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::{CurrencyUnit, KeySetInfo, Proofs, State, Token};
use cdk::wallet::WalletRepository;
use cdk_common::wallet::KeysetLoadPolicy;
use clap::Args;

use crate::utils::get_or_create_wallet;

/// Check proof state with a mint.
#[derive(Args)]
pub struct CheckStateSubCommand {
    /// Mint URL whose local wallet proofs should be checked.
    #[arg(long, conflicts_with = "token")]
    mint_url: Option<MintUrl>,
    /// Cashu token whose proofs should be checked.
    #[arg(long, conflicts_with = "mint_url")]
    token: Option<String>,
}

pub async fn check_state(
    wallet_repository: &WalletRepository,
    sub_command_args: &CheckStateSubCommand,
    default_unit: &CurrencyUnit,
) -> Result<()> {
    match (&sub_command_args.mint_url, &sub_command_args.token) {
        (Some(mint_url), None) => {
            check_local_wallet(wallet_repository, mint_url, default_unit).await
        }
        (None, Some(token)) => check_token(wallet_repository, token, default_unit).await,
        (None, None) => bail!("Provide either --mint-url or --token"),
        (Some(_), Some(_)) => bail!("Provide either --mint-url or --token, not both"),
    }
}

async fn check_local_wallet(
    wallet_repository: &WalletRepository,
    mint_url: &MintUrl,
    unit: &CurrencyUnit,
) -> Result<()> {
    let wallet = get_or_create_wallet(wallet_repository, mint_url, unit).await?;
    let proofs = wallet
        .get_proofs_by_states(vec![
            State::Unspent,
            State::Pending,
            State::Reserved,
            State::PendingSpent,
        ])
        .await?;

    if proofs.is_empty() {
        println!("No local proofs found for {mint_url} {unit}");
        return Ok(());
    }

    let states = wallet.check_proofs_spent(proofs).await?;
    print_states("Local proof states", &states);

    Ok(())
}

async fn check_token(
    wallet_repository: &WalletRepository,
    token_str: &str,
    default_unit: &CurrencyUnit,
) -> Result<()> {
    let token = Token::from_str(token_str)?;
    let mint_url = token.mint_url()?;
    let unit = token.unit().unwrap_or_else(|| default_unit.clone());
    let wallet = get_or_create_wallet(wallet_repository, &mint_url, &unit).await?;
    let keysets = wallet.keysets(KeysetLoadPolicy::CacheThenNetwork).await?;
    let keyset_infos = keysets
        .into_iter()
        .map(|keyset| KeySetInfo {
            id: keyset.id,
            unit: keyset.unit,
            active: keyset.active.unwrap_or(false),
            input_fee_ppk: keyset.input_fee_ppk,
            final_expiry: keyset.final_expiry,
        })
        .collect::<Vec<_>>();
    let proofs: Proofs = token.proofs(&keyset_infos)?;

    if proofs.is_empty() {
        println!("Token contains no proofs for {mint_url} {unit}");
        return Ok(());
    }

    let states = wallet.check_proofs_spent(proofs).await?;
    print_states("Token proof states", &states);

    Ok(())
}

fn print_states(label: &str, states: &[cdk::nuts::ProofState]) {
    let unspent = states
        .iter()
        .filter(|proof_state| proof_state.state == State::Unspent)
        .count();
    let pending = states
        .iter()
        .filter(|proof_state| proof_state.state == State::Pending)
        .count();
    let spent = states
        .iter()
        .filter(|proof_state| proof_state.state == State::Spent)
        .count();

    println!(
        "{label}: total={} unspent={unspent} pending={pending} spent={spent}",
        states.len()
    );
    for proof_state in states {
        println!("{} {}", proof_state.y, proof_state.state);
    }
}
