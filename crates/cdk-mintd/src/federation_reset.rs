//! Reset a member that holds federation state below every peer's retention
//! floor.
//!
//! Such a member cannot catch up: its peers deleted the history it needs, and
//! a state transfer installs only into an empty member. The running daemon
//! halts with the `reset_required` sync state. Its operator stops it, runs
//! this command, and starts it again; the member then installs a peer's
//! checkpoint state and catches up the rest.

use anyhow::{bail, Context, Result};
use cdk_common::database::DynMintDatabase;

use crate::cli::{CLIArgs, FederationResetMemberArgs};

pub(crate) async fn run_reset_member_command(
    cli_args: &CLIArgs,
    args: &FederationResetMemberArgs,
) -> Result<()> {
    if !args.confirm_wipe {
        bail!(
            "federation reset-member deletes this member's federation consensus state and its \
             quotes, proofs and signatures; stop the daemon first and pass --confirm-wipe to \
             proceed"
        );
    }
    let work_dir = crate::get_work_directory(cli_args).await?;
    let settings = crate::load_stored_settings(&work_dir, cli_args).await?;
    if !settings
        .federation
        .as_ref()
        .is_some_and(|federation| federation.enabled)
    {
        bail!("federation reset-member requires a federation-enabled mint configuration");
    }

    #[cfg(feature = "sqlcipher")]
    let password = cli_args.password.clone();
    #[cfg(not(feature = "sqlcipher"))]
    let password = None;
    let crate::MintDatabases { localstore, .. } =
        crate::setup_database(&settings, &work_dir, password)
            .await
            .context("failed to open mint database for federation member reset")?;
    let deleted = reset_federation_member(&localstore).await?;
    println!("federation_member_reset=true");
    println!("deleted_rows={deleted}");
    println!(
        "Start the member again: it installs a peer's checkpoint state and catches up. \
         Key material, configuration and the FROST nonce store were kept."
    );
    Ok(())
}

/// Wipe the member's federation and mint state in one transaction.
pub(crate) async fn reset_federation_member(db: &DynMintDatabase) -> Result<u64> {
    let mut tx = db
        .begin_transaction()
        .await
        .context("failed to begin federation member reset")?;
    let deleted = tx
        .reset_federation_member_state()
        .await
        .context("failed to wipe federation member state")?;
    tx.commit()
        .await
        .context("failed to commit federation member reset")?;
    let frontier = db.get_federation_checkpoint_frontier().await?;
    if frontier != cdk_common::federation::FederationCheckpointFrontier::genesis() {
        bail!("federation member reset left a non-genesis checkpoint frontier");
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use crate::cli::{CLIArgs, Commands, FederationCommand};

    #[tokio::test]
    async fn reset_member_refuses_without_confirmation() {
        let args = CLIArgs::try_parse_from(["cdk-mintd", "federation", "reset-member"])
            .expect("reset command parses");
        let Some(Commands::Federation(federation)) = &args.command else {
            panic!("federation command");
        };
        let FederationCommand::ResetMember(reset) = &federation.command else {
            panic!("reset-member command");
        };
        assert!(!reset.confirm_wipe);
        let refused = super::run_reset_member_command(&args, reset)
            .await
            .expect_err("reset without confirmation is refused");
        assert!(refused.to_string().contains("--confirm-wipe"));

        let confirmed =
            CLIArgs::try_parse_from(["cdk-mintd", "federation", "reset-member", "--confirm-wipe"])
                .expect("confirmed reset parses");
        let Some(Commands::Federation(federation)) = &confirmed.command else {
            panic!("federation command");
        };
        let FederationCommand::ResetMember(reset) = &federation.command else {
            panic!("reset-member command");
        };
        assert!(reset.confirm_wipe);
    }

    #[cfg(feature = "sqlite")]
    #[tokio::test]
    async fn reset_keeps_key_material_and_leaves_an_empty_genesis_member() {
        use std::sync::Arc;

        use cdk_common::database::DynMintDatabase;

        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory mint database"),
        );
        let mut tx = db.begin_transaction().await.expect("begin");
        tx.kv_write("cdk_mint", "config", "kept", b"configuration")
            .await
            .expect("keep configuration row");
        tx.kv_write("federation", "chain_observation", "utxo_x", b"{}")
            .await
            .expect("projection row");
        tx.kv_write("cdk_bark", "replay_next_index", "scope", b"7")
            .await
            .expect("replay marker");
        tx.commit().await.expect("commit");

        assert_eq!(
            super::reset_federation_member(&db)
                .await
                .expect("reset an empty member"),
            2
        );
        assert_eq!(
            db.kv_read("cdk_mint", "config", "kept")
                .await
                .expect("configuration read")
                .as_deref(),
            Some(b"configuration".as_slice())
        );
        assert!(db
            .kv_entries("federation", "chain_observation")
            .await
            .expect("projection rows")
            .is_empty());
        assert_eq!(
            db.get_federation_checkpoint_frontier()
                .await
                .expect("frontier"),
            cdk_common::federation::FederationCheckpointFrontier::genesis()
        );
    }
}
