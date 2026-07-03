use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, MintQuoteState, PaymentMethod, State};
use cdk::wallet::{FederatedMintConnector, HttpClient, MintConnector, Wallet, WalletBuilder};
use cdk::{Amount, Error};
use cdk_common::federation::{FederationConfig, MemberId};
use cdk_fake_wallet::create_fake_invoice;

use crate::cli::FederationSmokeOperationsArgs;

pub(crate) async fn run_smoke_operations_command(
    args: &FederationSmokeOperationsArgs,
) -> Result<()> {
    let public_config: FederationConfig =
        crate::read_federation_json(&args.public_config, "public federation config")?;
    public_config
        .validate()
        .context("invalid public federation config")?;

    let unit = CurrencyUnit::from_str(&args.unit)
        .with_context(|| format!("invalid federation smoke unit {}", args.unit))?;
    let member_connectors = member_connectors(&public_config);
    let federation_connector =
        FederatedMintConnector::new(public_config.clone(), member_connectors.clone())
            .context("federated wallet connector")?;
    let connector = Arc::new(federation_connector);
    let seed = [7u8; 64];
    let wallet = smoke_wallet(&public_config, unit.clone(), seed, connector.clone()).await?;

    let quote = wallet
        .mint_quote(
            PaymentMethod::BOLT11,
            Some(Amount::from(args.mint_amount)),
            None,
            None,
        )
        .await
        .context("create federation mint quote")?;
    wait_for_mint_quote_paid(
        &member_connectors,
        &quote.id,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
    )
    .await?;

    let minted = wallet
        .mint(&quote.id, SplitTarget::default(), None)
        .await
        .context("mint through federation connector")?;
    let minted_amount = minted.total_amount()?;
    if minted_amount != Amount::from(args.mint_amount) {
        bail!(
            "federation smoke minted {}, expected {}",
            minted_amount,
            args.mint_amount
        );
    }

    let proofs_before_swap = wallet
        .get_unspent_proofs()
        .await
        .context("load minted proofs")?;
    wallet
        .swap(
            None,
            SplitTarget::default(),
            proofs_before_swap.clone(),
            None,
            false,
            false,
        )
        .await
        .context("swap through federation connector")?;
    let proofs_after_swap = wallet
        .get_unspent_proofs()
        .await
        .context("load swapped proofs")?;
    let swapped_amount = proofs_after_swap.total_amount()?;
    if swapped_amount != minted_amount {
        bail!(
            "federation smoke swapped {}, expected {}",
            swapped_amount,
            minted_amount
        );
    }

    let spent_states = wallet
        .check_proofs_spent(proofs_before_swap)
        .await
        .context("check spent state for swapped inputs")?;
    if spent_states
        .iter()
        .any(|proof_state| proof_state.state != State::Spent)
    {
        bail!("federation smoke check-state did not mark swapped inputs spent");
    }

    let melt_invoice = create_fake_invoice(1000, "federation smoke melt".to_string());
    let melt_quote = wallet
        .melt_quote(PaymentMethod::BOLT11, melt_invoice, None, None)
        .await
        .context("create federation melt quote")?;
    wait_for_melt_quote_visible(
        &member_connectors,
        &melt_quote.id,
        args.quote_retries,
        Duration::from_millis(args.quote_sleep_ms),
    )
    .await?;

    let finalized = wallet
        .prepare_melt(&melt_quote.id, HashMap::new())
        .await
        .context("prepare federation melt")?
        .confirm()
        .await
        .context("confirm federation melt")?;
    if finalized.state() != MeltQuoteState::Paid {
        bail!(
            "federation smoke melt ended in state {}, expected paid",
            finalized.state()
        );
    }

    let restored_wallet = smoke_wallet(&public_config, unit, seed, connector).await?;
    let restored = restored_wallet.restore().await.context("restore wallet")?;
    if restored.unspent == Amount::ZERO
        && restored.spent == Amount::ZERO
        && restored.pending == Amount::ZERO
    {
        bail!("federation smoke restore recovered no proofs");
    }

    println!(
        "federation operation smoke passed: minted={} swapped={} melted={} fee={} restored_unspent={} restored_spent={} restored_pending={}",
        minted_amount,
        swapped_amount,
        finalized.amount(),
        finalized.fee_paid(),
        restored.unspent,
        restored.spent,
        restored.pending
    );

    Ok(())
}

fn member_connectors(
    public_config: &FederationConfig,
) -> BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>> {
    public_config
        .members
        .iter()
        .map(|(member_id, member)| {
            let client = HttpClient::new(member.public_mint_url.clone(), None);
            (
                *member_id,
                Arc::new(client) as Arc<dyn MintConnector + Send + Sync>,
            )
        })
        .collect()
}

async fn smoke_wallet(
    public_config: &FederationConfig,
    unit: CurrencyUnit,
    seed: [u8; 64],
    connector: Arc<FederatedMintConnector>,
) -> Result<Wallet, Error> {
    let primary_member = public_config
        .members
        .values()
        .next()
        .ok_or_else(|| Error::Custom("empty federation roster".to_string()))?;
    let localstore = Arc::new(cdk_sqlite::wallet::memory::empty().await?);

    WalletBuilder::new()
        .mint_url(primary_member.public_mint_url.clone())
        .unit(unit)
        .localstore(localstore)
        .seed(seed)
        .target_proof_count(3)
        .shared_client(connector)
        .build()
}

async fn wait_for_mint_quote_paid(
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    quote_id: &str,
    retries: u16,
    sleep: Duration,
) -> Result<()> {
    for _ in 0..retries {
        let mut all_paid = true;
        for (member_id, connector) in member_connectors {
            match connector
                .get_mint_quote_status(PaymentMethod::BOLT11, quote_id)
                .await
            {
                Ok(status) if status.state() == Some(MintQuoteState::Paid) => {}
                Ok(status) => {
                    all_paid = false;
                    tracing::debug!(
                        member_id = %member_id,
                        state = ?status.state(),
                        "federation smoke mint quote is not paid yet"
                    );
                }
                Err(err) => {
                    all_paid = false;
                    tracing::debug!(
                        member_id = %member_id,
                        "federation smoke mint quote status unavailable: {}",
                        err
                    );
                }
            }
        }
        if all_paid {
            return Ok(());
        }
        tokio::time::sleep(sleep).await;
    }

    bail!(
        "federation smoke mint quote {} was not paid on every member after {} attempts",
        quote_id,
        retries
    )
}

async fn wait_for_melt_quote_visible(
    member_connectors: &BTreeMap<MemberId, Arc<dyn MintConnector + Send + Sync>>,
    quote_id: &str,
    retries: u16,
    sleep: Duration,
) -> Result<()> {
    for _ in 0..retries {
        let mut all_visible = true;
        for (member_id, connector) in member_connectors {
            if let Err(err) = connector
                .get_melt_quote_status(PaymentMethod::BOLT11, quote_id)
                .await
            {
                all_visible = false;
                tracing::debug!(
                    member_id = %member_id,
                    "federation smoke melt quote status unavailable: {}",
                    err
                );
            }
        }
        if all_visible {
            return Ok(());
        }
        tokio::time::sleep(sleep).await;
    }

    bail!(
        "federation smoke melt quote {} was not visible on every member after {} attempts",
        quote_id,
        retries
    )
}
