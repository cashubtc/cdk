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

#[cfg(all(test, feature = "federation"))]
mod tests {
    use std::sync::Arc;

    use cdk::nuts::Proof;
    use cdk::secret::Secret;
    use cdk::wallet::{WalletConfig, WalletRepositoryBuilder};
    use cdk::Amount;
    use cdk_common::database::WalletDatabase;
    use cdk_common::wallet::ProofInfo;

    use super::*;
    use crate::test_support::{federated_connector_with_faults, test_bls_signature, MemberFault};

    #[tokio::test]
    async fn command_rejects_check_state_without_threshold_member_agreement() {
        let fixture = federated_connector_with_faults([
            MemberFault::CheckState(State::Unspent),
            MemberFault::CheckState(State::Pending),
            MemberFault::CheckState(State::Spent),
        ]);
        let localstore: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync> =
            Arc::new(
                cdk_sqlite::wallet::memory::empty()
                    .await
                    .expect("wallet database"),
            );
        let repository = WalletRepositoryBuilder::new()
            .localstore(localstore.clone())
            .seed([0u8; 64])
            .build()
            .await
            .expect("wallet repository");
        repository
            .set_mint_config(
                fixture.mint_url.clone(),
                CurrencyUnit::Sat,
                WalletConfig::new().with_mint_connector(fixture.connector),
            )
            .await
            .expect("federation wallet");
        let proof = Proof::new(
            Amount::from(1),
            fixture.keyset_id,
            Secret::generate(),
            test_bls_signature(b"cli-check-state-proof"),
        );
        localstore
            .update_proofs(
                vec![ProofInfo::new(
                    proof,
                    fixture.mint_url.clone(),
                    State::Unspent,
                    CurrencyUnit::Sat,
                )
                .expect("proof info")],
                Vec::new(),
            )
            .await
            .expect("store proof");

        let err = check_state(
            &repository,
            &CheckStateSubCommand {
                mint_url: Some(fixture.mint_url),
                token: None,
            },
            &CurrencyUnit::Sat,
        )
        .await
        .expect_err("mismatched federation states must fail");

        assert!(err
            .to_string()
            .contains("insufficient matching federation check-state responses: required 2"));
    }
}
