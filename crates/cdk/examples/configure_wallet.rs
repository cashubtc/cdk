//! Configure per-mint wallet behavior through `WalletManager`.

use std::sync::Arc;
use std::time::Duration;

use cdk::nuts::CurrencyUnit;
use cdk::wallet::advanced::MintAdvancedOptions;
use cdk::wallet::{
    MintRegistrationRequest, WalletConfigurationRequest, WalletIdentity, WalletManagerBuilder,
};
use cdk_sqlite::wallet::memory;
use rand::random;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manager = WalletManagerBuilder::new()
        .with_store(Arc::new(memory::empty().await?))
        .with_seed(random::<[u8; 64]>())
        .build()
        .await?;

    let primary = "https://testnut.cashudevkit.org".parse()?;
    let primary_config =
        MintAdvancedOptions::new().with_metadata_cache_ttl(Duration::from_secs(600));
    let wallet = manager
        .configure_wallet(
            WalletConfigurationRequest::new(WalletIdentity::new(primary, CurrencyUnit::Sat))
                .with_advanced(primary_config),
        )
        .await?;
    println!(
        "Configured {} with a 10-minute metadata cache",
        wallet.identity().mint_url
    );

    let secondary: cdk::mint_url::MintUrl = "https://testnut.cashu.space".parse()?;
    manager
        .register_mint(
            MintRegistrationRequest::new(secondary.clone())
                .with_advanced(MintAdvancedOptions::new().without_metadata_cache_expiry()),
        )
        .await?;
    println!("Configured {secondary} with explicit-refresh metadata");

    Ok(())
}
