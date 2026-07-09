use std::io::{self, Write};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "federation")]
use std::sync::{Arc, OnceLock, RwLock};

#[cfg(feature = "federation")]
use anyhow::anyhow;
use anyhow::{bail, Result};
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
#[cfg(feature = "federation")]
use cdk::wallet::{Wallet, WalletConfig};

#[cfg(feature = "federation")]
use crate::sub_commands::federation::{FederationWalletRegistry, StoredFederationWallet};

static NON_INTERACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "federation")]
static FEDERATION_REGISTRY: OnceLock<RwLock<Option<Arc<FederationWalletRegistry>>>> =
    OnceLock::new();

pub fn set_non_interactive(enabled: bool) {
    NON_INTERACTIVE.store(enabled, Ordering::Relaxed);
}

/// Helper function to get user input with a prompt
pub fn get_user_input(prompt: &str) -> Result<String> {
    if NON_INTERACTIVE.load(Ordering::Relaxed) {
        bail!(
            "Interactive input is disabled (--non-interactive). Missing required argument for prompt: {prompt}"
        );
    }

    println!("{prompt}");
    let mut user_input = String::new();
    io::stdout().flush()?;
    io::stdin().read_line(&mut user_input)?;
    Ok(user_input.trim().to_string())
}

/// Helper function to get a number from user input with a prompt
pub fn get_number_input<T>(prompt: &str) -> Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    let input = get_user_input(prompt)?;
    let number = input.parse::<T>()?;
    Ok(number)
}

/// Helper function to get an existing wallet or create one if it doesn't exist
pub async fn get_or_create_wallet(
    wallet_repository: &WalletRepository,
    mint_url: &MintUrl,
    unit: &CurrencyUnit,
) -> Result<cdk::wallet::Wallet> {
    #[cfg(feature = "federation")]
    if let Some(federation_wallet) = configured_federation_wallet(mint_url)? {
        return create_or_replace_federation_wallet(
            wallet_repository,
            mint_url,
            unit,
            &federation_wallet,
        )
        .await;
    }

    match wallet_repository.get_wallet(mint_url, unit).await {
        Ok(wallet) => Ok(wallet),
        Err(_) => wallet_repository
            .create_wallet(mint_url.clone(), unit.clone(), None)
            .await
            .map_err(Into::into),
    }
}

#[cfg(feature = "federation")]
/// Install the in-memory federation registry used by wallet helpers.
pub fn set_federation_registry(registry: FederationWalletRegistry) -> Result<()> {
    let lock = FEDERATION_REGISTRY.get_or_init(|| RwLock::new(None));
    let mut guard = lock
        .write()
        .map_err(|_| anyhow!("Federation registry lock is poisoned"))?;
    *guard = Some(Arc::new(registry));
    Ok(())
}

#[cfg(feature = "federation")]
/// Activate all imported federation wallet configs in a wallet repository.
pub async fn activate_federation_wallets(wallet_repository: &WalletRepository) -> Result<()> {
    let Some(registry) = federation_registry()? else {
        return Ok(());
    };

    for wallet in &registry.wallets {
        activate_federation_wallet(wallet_repository, wallet).await?;
    }

    Ok(())
}

#[cfg(feature = "federation")]
/// Activate one imported federation wallet config in a wallet repository.
pub async fn activate_federation_wallet(
    wallet_repository: &WalletRepository,
    federation_wallet: &StoredFederationWallet,
) -> Result<()> {
    for unit in federation_wallet.active_units() {
        create_or_replace_federation_wallet(
            wallet_repository,
            &federation_wallet.local_mint_url,
            &unit,
            federation_wallet,
        )
        .await?;
    }

    Ok(())
}

#[cfg(feature = "federation")]
fn federation_registry() -> Result<Option<Arc<FederationWalletRegistry>>> {
    let Some(lock) = FEDERATION_REGISTRY.get() else {
        return Ok(None);
    };
    let guard = lock
        .read()
        .map_err(|_| anyhow!("Federation registry lock is poisoned"))?;
    Ok(guard.clone())
}

#[cfg(feature = "federation")]
fn configured_federation_wallet(mint_url: &MintUrl) -> Result<Option<StoredFederationWallet>> {
    let Some(registry) = federation_registry()? else {
        return Ok(None);
    };
    Ok(registry.find_by_mint_url(mint_url).cloned())
}

#[cfg(feature = "federation")]
async fn create_or_replace_federation_wallet(
    wallet_repository: &WalletRepository,
    mint_url: &MintUrl,
    unit: &CurrencyUnit,
    federation_wallet: &StoredFederationWallet,
) -> Result<Wallet> {
    if !federation_wallet.supports_unit(unit) {
        bail!(
            "Federation {} does not advertise an active {} keyset",
            federation_wallet.federation_id(),
            unit
        );
    }

    let config: WalletConfig = federation_wallet.wallet_config()?;
    wallet_repository
        .set_mint_config(mint_url.clone(), unit.clone(), config)
        .await
        .map_err(Into::into)
}
