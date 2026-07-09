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
    if let Some(federation_wallet) = federation_wallet_for_mint_url(mint_url)? {
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
/// Return whether a mint URL maps to an imported federation wallet.
pub fn is_configured_federation_mint_url(mint_url: &MintUrl) -> Result<bool> {
    Ok(federation_wallet_for_mint_url(mint_url)?.is_some())
}

#[cfg(feature = "federation")]
/// Return the imported federation wallet that matches a local or member mint URL.
pub fn federation_wallet_for_mint_url(
    mint_url: &MintUrl,
) -> Result<Option<StoredFederationWallet>> {
    let Some(registry) = federation_registry()? else {
        return Ok(None);
    };
    Ok(registry.find_by_mint_url(mint_url).cloned())
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

#[cfg(all(test, feature = "federation"))]
mod federation_tests {
    use std::collections::BTreeMap;
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk::cdk_database::{self, WalletDatabase};
    use cdk::nuts::nut01::BlsSecretKey;
    use cdk::wallet::WalletRepositoryBuilder;
    use cdk::Amount;
    use cdk_common::federation::{
        generate_trusted_dealer_configs, MemberId, ThresholdParams, TrustedDealerKeysetSpec,
        TrustedDealerMember,
    };
    use cdk_common::SecretKey;

    use super::*;
    use crate::sub_commands::federation::{
        FederationWalletRegistry, StoredFederationConnectorPolicy,
    };

    fn member_id(id: u16) -> MemberId {
        MemberId::new(id).expect("valid member id")
    }

    fn identity_secret(member_id: MemberId) -> SecretKey {
        let mut bytes = [0u8; 32];
        bytes[31] = member_id.get() as u8;
        SecretKey::from_slice(&bytes).expect("valid secp256k1 secret")
    }

    fn member(member_id: MemberId) -> TrustedDealerMember {
        TrustedDealerMember::new(
            member_id,
            MintUrl::from_str(&format!("https://mint{}.example.com", member_id.get()))
                .expect("public mint url"),
            MintUrl::from_str(&format!("https://fed{}.example.com", member_id.get()))
                .expect("federation api url"),
            identity_secret(member_id),
        )
    }

    fn bls_key(byte: u8) -> BlsSecretKey {
        BlsSecretKey::from_reduced_bytes(&[byte; 32])
    }

    fn valid_public_config() -> cdk_common::federation::FederationConfig {
        let params = ThresholdParams::with_bft_consensus(3, 2).expect("params");
        generate_trusted_dealer_configs(
            params,
            vec![
                member(member_id(1)),
                member(member_id(2)),
                member(member_id(3)),
            ],
            vec![TrustedDealerKeysetSpec::new(
                CurrencyUnit::Sat,
                true,
                0,
                None,
                BTreeMap::from([
                    (Amount::from(1), vec![bls_key(1), bls_key(2)]),
                    (Amount::from(2), vec![bls_key(3), bls_key(4)]),
                ]),
            )],
        )
        .expect("trusted dealer configs")
        .public_config
    }

    async fn test_repository() -> WalletRepository {
        let localstore: Arc<dyn WalletDatabase<cdk_database::Error> + Send + Sync> = Arc::new(
            cdk_sqlite::wallet::memory::empty()
                .await
                .expect("wallet memory db"),
        );
        WalletRepositoryBuilder::new()
            .localstore(localstore)
            .seed([0u8; 64])
            .build()
            .await
            .expect("wallet repository")
    }

    fn connector_debug(wallet: &cdk::wallet::Wallet) -> String {
        format!("{:?}", wallet.mint_connector())
    }

    #[tokio::test]
    async fn get_or_create_wallet_uses_federation_registry_for_local_and_member_urls() {
        let public_config = valid_public_config();
        let local_mint_url = public_config
            .members
            .get(&member_id(1))
            .expect("member one")
            .public_mint_url
            .clone();
        let member_two_url = public_config
            .members
            .get(&member_id(2))
            .expect("member two")
            .public_mint_url
            .clone();
        let standalone_url =
            MintUrl::from_str("https://standalone.example.com").expect("standalone url");

        let mut registry = FederationWalletRegistry::default();
        registry
            .upsert(
                public_config,
                local_mint_url.clone(),
                StoredFederationConnectorPolicy::default(),
            )
            .expect("upsert");
        set_federation_registry(registry).expect("set federation registry");

        let repository = test_repository().await;
        activate_federation_wallets(&repository)
            .await
            .expect("activate federation wallets");

        let local_wallet = repository
            .get_wallet(&local_mint_url, &CurrencyUnit::Sat)
            .await
            .expect("pre-activated local federation wallet");
        assert!(connector_debug(&local_wallet).contains("FederatedMintConnector"));

        let member_wallet = get_or_create_wallet(&repository, &member_two_url, &CurrencyUnit::Sat)
            .await
            .expect("member url federation wallet");
        assert_eq!(member_wallet.mint_url, member_two_url);
        assert!(connector_debug(&member_wallet).contains("FederatedMintConnector"));

        let unsupported_unit_err =
            get_or_create_wallet(&repository, &local_mint_url, &CurrencyUnit::Usd)
                .await
                .expect_err("unsupported federation unit rejected");
        let unsupported_unit_err = unsupported_unit_err.to_string();
        assert!(unsupported_unit_err.contains("does not advertise an active"));
        assert!(unsupported_unit_err.contains("keyset"));

        let standalone_wallet =
            get_or_create_wallet(&repository, &standalone_url, &CurrencyUnit::Sat)
                .await
                .expect("standalone wallet");
        assert_eq!(standalone_wallet.mint_url, standalone_url);
        assert!(!connector_debug(&standalone_wallet).contains("FederatedMintConnector"));
    }
}
