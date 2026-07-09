use anyhow::Result;
use cdk::mint_url::MintUrl;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::WalletRepository;
use clap::Args;

use crate::utils::get_or_create_wallet;

#[derive(Args)]
pub struct RestoreSubCommand {
    /// Mint Url
    mint_url: MintUrl,
}

pub async fn restore(
    wallet_repository: &WalletRepository,
    sub_command_args: &RestoreSubCommand,
    unit: &CurrencyUnit,
) -> Result<()> {
    let mint_url = sub_command_args.mint_url.clone();

    let wallet = get_or_create_wallet(wallet_repository, &mint_url, unit).await?;

    let restored = wallet.restore().await?;

    println!("Restored: {}", restored.unspent);
    println!("Spent: {}", restored.spent);
    println!("Pending: {}", restored.pending);

    Ok(())
}

#[cfg(all(test, feature = "federation"))]
mod tests {
    use std::sync::Arc;

    use cdk::wallet::{WalletConfig, WalletRepositoryBuilder};
    use cdk_common::database::WalletDatabase;

    use super::*;
    use crate::test_support::{federated_connector_with_faults, MemberFault};

    #[tokio::test]
    async fn command_rejects_restore_without_threshold_member_agreement() {
        let fixture = federated_connector_with_faults([
            MemberFault::RestoreOutput(None),
            MemberFault::RestoreOutput(Some(0)),
            MemberFault::RestoreOutput(Some(1)),
        ]);
        let localstore: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync> =
            Arc::new(
                cdk_sqlite::wallet::memory::empty()
                    .await
                    .expect("wallet database"),
            );
        let repository = WalletRepositoryBuilder::new()
            .localstore(localstore)
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

        let err = restore(
            &repository,
            &RestoreSubCommand {
                mint_url: fixture.mint_url,
            },
            &CurrencyUnit::Sat,
        )
        .await
        .expect_err("mismatched federation restore views must fail");

        assert!(err
            .to_string()
            .contains("insufficient matching federation restore responses: required 2"));
    }
}
