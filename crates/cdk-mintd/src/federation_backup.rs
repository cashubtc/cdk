use anyhow::{bail, Context, Result};
use cdk_common::database::DynMintDatabase;
use cdk_common::federation::{
    FederationConfig, FederationDatabaseBackupManifest, MemberConfigPrivate,
};

use crate::cli::{CLIArgs, FederationBackupManifestArgs};

pub(crate) async fn run_backup_manifest_command(
    cli_args: &CLIArgs,
    args: &FederationBackupManifestArgs,
) -> Result<()> {
    let work_dir = crate::get_work_directory(cli_args).await?;
    let settings = crate::load_stored_settings(&work_dir, cli_args).await?;
    let (public_config_path, private_config_path) = crate::federation_config_paths(
        cli_args,
        &args.public_config,
        &args.private_config,
        "federation backup-manifest",
    )
    .await?;
    crate::validate_private_federation_config_file(&private_config_path)?;
    let public_config: FederationConfig =
        crate::read_federation_json(&public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(&private_config_path, "private federation config")?;

    #[cfg(feature = "sqlcipher")]
    let password = cli_args.password.clone();
    #[cfg(not(feature = "sqlcipher"))]
    let password = None;
    let (db, _, _, _) = crate::setup_database(&settings, &work_dir, password)
        .await
        .context("failed to open mint database for federation backup manifest")?;
    let manifest = build_manifest(&db, &public_config, &private_config).await?;
    crate::federation_files::write_private_json_file(&args.output, &manifest, args.force)?;
    println!("backup_manifest_written=true");
    Ok(())
}

pub(crate) async fn validate_configured_restore(
    settings: &crate::config::Settings,
    db: &DynMintDatabase,
) -> Result<()> {
    let Some(federation) = settings.federation.as_ref() else {
        return Ok(());
    };
    if !federation.enabled {
        return Ok(());
    }
    let Some(manifest_path) = federation.restore_manifest_path.as_deref() else {
        return Ok(());
    };
    let public_config_path = federation.public_config_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "federation.public_config_path must be set when restore validation is enabled"
        )
    })?;
    let private_config_path = federation.private_config_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "federation.private_config_path must be set when restore validation is enabled"
        )
    })?;

    crate::validate_private_federation_config_file(private_config_path)?;
    let public_config: FederationConfig =
        crate::read_federation_json(public_config_path, "public federation config")?;
    let private_config: MemberConfigPrivate =
        crate::read_federation_json(private_config_path, "private federation config")?;
    let expected: FederationDatabaseBackupManifest =
        crate::read_federation_json(manifest_path, "federation database backup manifest")?;
    expected.validate_shape()?;
    let current = build_manifest(db, &public_config, &private_config).await?;
    expected
        .ensure_matches_current(&current)
        .context("restored federation database validation failed")?;
    tracing::info!(
        federation_id = %current.federation_id,
        member_id = %current.member_id,
        checkpoint_id = %current.trusted_checkpoint_id,
        "Validated restored federation database against its checkpoint-bound backup manifest"
    );
    Ok(())
}

async fn build_manifest(
    db: &DynMintDatabase,
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
) -> Result<FederationDatabaseBackupManifest> {
    public_config.validate()?;
    private_config
        .validate_against_public(public_config)
        .context("private federation config does not match public federation config")?;
    let expected_config_digest = public_config.config_digest_for_validated_config()?;
    let snapshot = db
        .get_federation_config(public_config.federation_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("restored database has no federation config snapshot"))?;
    if snapshot.config_digest != expected_config_digest || snapshot.config != *public_config {
        bail!("restored database federation config does not match the supplied public config");
    }

    let frontier = db.get_federation_checkpoint_frontier().await?;
    let quorum = db
        .get_trusted_federation_checkpoint_quorum()
        .await?
        .ok_or_else(|| anyhow::anyhow!("restored database has no trusted federation checkpoint"))?;
    let verified = quorum
        .verify(public_config)
        .context("restored database trusted checkpoint is invalid")?;
    let active_session = db
        .get_federation_aleph_bft_active_session()
        .await?
        .ok_or_else(|| anyhow::anyhow!("restored database has no active AlephBFT session"))?;
    let backup = db
        .get_federation_aleph_bft_backup_metadata(active_session)
        .await?;

    FederationDatabaseBackupManifest::new(
        public_config,
        private_config,
        &verified,
        frontier,
        active_session,
        backup.chunk_count,
        backup.total_bytes,
    )
    .context("restored database cannot form a valid federation backup manifest")
}

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use std::str::FromStr;
    use std::sync::Arc;

    use cdk_common::federation::{
        generate_trusted_dealer_configs, FederationCheckpointBoundary, FederationCheckpointQuorum,
        FederationCheckpointSignature, FederationConfigDigest, FederationConfigSnapshot,
        FederationConsensusRuntime, FederationDatabaseBackupManifest, FederationId,
        FederationMaterializedStateDigest, FederationRecoveryBarrier, MemberConfigPrivate,
        MemberId, ThresholdParams, TrustedDealerMember,
    };
    use cdk_common::mint_url::MintUrl;
    use cdk_common::SecretKey;

    use super::*;

    async fn backup_fixture() -> (
        DynMintDatabase,
        FederationConfig,
        MemberConfigPrivate,
        FederationDatabaseBackupManifest,
    ) {
        let members = [1_u16, 2]
            .into_iter()
            .map(|id| {
                let member_id = MemberId::new(id).expect("member id");
                TrustedDealerMember::new(
                    member_id,
                    MintUrl::from_str(&format!("https://mint{id}.example.invalid"))
                        .expect("mint url"),
                    MintUrl::from_str(&format!("https://fed{id}.example.invalid"))
                        .expect("federation url"),
                    SecretKey::from_slice(&[id as u8; 32]).expect("identity secret"),
                )
            })
            .collect();
        let bundle = generate_trusted_dealer_configs(
            ThresholdParams::new(2, 2, 2).expect("thresholds"),
            members,
            Vec::new(),
        )
        .expect("federation config");
        let member_id = MemberId::new(1).expect("member one");
        let private_config = bundle
            .private_configs
            .get(&member_id)
            .expect("private config")
            .clone();
        let mut runtime = FederationConsensusRuntime::authenticated_aleph_bft(
            &bundle.public_config,
            member_id,
            0,
        )
        .expect("consensus runtime");
        let barrier =
            FederationRecoveryBarrier::new(bundle.public_config.federation_id, 0, None, 0)
                .expect("recovery barrier");
        assert!(runtime
            .submit_aleph_bft_recovery_barrier(barrier)
            .expect("submit barrier"));
        let payload = runtime
            .next_aleph_bft_unit_payload()
            .expect("unit payload")
            .expect("barrier unit");
        runtime
            .finalize_aleph_bft_unit(payload)
            .expect("finalize barrier");

        let db: DynMintDatabase = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("mint database"),
        );
        let mut tx = db.begin_transaction().await.expect("begin transaction");
        tx.set_federation_config(
            &FederationConfigSnapshot::new(bundle.public_config.clone()).expect("config snapshot"),
        )
        .await
        .expect("persist config");
        tx.add_federation_aleph_bft_finalized_items(
            runtime
                .aleph_bft_finalized_items()
                .expect("finalized items"),
        )
        .await
        .expect("persist finalized items");
        tx.set_federation_aleph_bft_active_session(0)
            .await
            .expect("persist active session");
        tx.replace_federation_aleph_bft_backup_chunks(0, &[vec![1, 2, 3]])
            .await
            .expect("persist active backup");
        tx.commit().await.expect("commit history");

        let frontier = db
            .get_federation_checkpoint_frontier()
            .await
            .expect("checkpoint frontier");
        let boundary = FederationCheckpointBoundary::from_complete_unit(
            &frontier,
            runtime.session(),
            runtime.status().next_unit_finalization_index,
            runtime.next_unit_index_by_member().clone(),
        )
        .expect("checkpoint boundary");
        let checkpoint = boundary
            .recovery_checkpoint(
                &bundle.public_config,
                None,
                FederationMaterializedStateDigest::from_bytes([7; 32]),
            )
            .expect("recovery checkpoint");
        let signatures = bundle
            .private_configs
            .values()
            .map(|private| {
                FederationCheckpointSignature::new_signed_no_aux_rand(&checkpoint, private)
                    .expect("checkpoint signature")
            })
            .collect();
        let quorum = FederationCheckpointQuorum::new(checkpoint, signatures);
        let verified = quorum
            .verify(&bundle.public_config)
            .expect("checkpoint quorum");
        let mut tx = db.begin_transaction().await.expect("begin promotion");
        tx.promote_verified_federation_checkpoint(&verified)
            .await
            .expect("promote checkpoint");
        tx.commit().await.expect("commit promotion");

        let manifest = build_manifest(&db, &bundle.public_config, &private_config)
            .await
            .expect("backup manifest");
        (db, bundle.public_config, private_config, manifest)
    }

    #[tokio::test]
    async fn configured_restore_validates_before_runtime_startup() {
        let (db, public_config, private_config, manifest) = backup_fixture().await;
        assert_eq!(manifest.active_backup_chunk_count, 1);
        assert_eq!(manifest.active_backup_total_bytes, 3);

        let base = std::env::temp_dir().join(format!(
            "cdk_federation_backup_restore_{}",
            std::process::id()
        ));
        let public_path = base.with_extension("public.json");
        let private_path = base.with_extension("private.json");
        let manifest_path = base.with_extension("manifest.json");
        crate::federation_files::write_json_file(&public_path, &public_config, true)
            .expect("write public config");
        crate::federation_files::write_private_json_file(&private_path, &private_config, true)
            .expect("write private config");
        crate::federation_files::write_private_json_file(&manifest_path, &manifest, true)
            .expect("write manifest");
        let settings = crate::config::Settings {
            federation: Some(crate::config::Federation {
                enabled: true,
                public_config_path: Some(public_path.clone()),
                private_config_path: Some(private_path.clone()),
                restore_manifest_path: Some(manifest_path.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        validate_configured_restore(&settings, &db)
            .await
            .expect("matching restore");

        let mut wrong_federation = manifest.clone();
        wrong_federation.federation_id = FederationId::from_bytes([21; 32]);
        let mut wrong_config = manifest.clone();
        wrong_config.federation_config_digest = FederationConfigDigest::from_bytes([22; 32]);
        let mut wrong_checkpoint = manifest.clone();
        wrong_checkpoint.trusted_checkpoint_id =
            cdk_common::federation::FederationCheckpointId::from_bytes([23; 32]);
        wrong_checkpoint.durable_frontier.trusted_checkpoint_id =
            Some(wrong_checkpoint.trusted_checkpoint_id);
        let mut wrong_schema = manifest.clone();
        wrong_schema.database_schema_version += 1;
        let mut wrong_secrets = manifest;
        wrong_secrets.secret_storage_generation = cdk_common::util::hex::encode([24; 32]);
        for (label, wrong) in [
            ("federation", wrong_federation),
            ("config", wrong_config),
            ("checkpoint", wrong_checkpoint),
            ("schema", wrong_schema),
            ("secret generation", wrong_secrets),
        ] {
            crate::federation_files::write_private_json_file(&manifest_path, &wrong, true)
                .expect("replace manifest");
            let result = validate_configured_restore(&settings, &db).await;
            assert!(
                result.is_err(),
                "wrong {label} accepted before runtime startup"
            );
        }

        let _ = std::fs::remove_file(public_path);
        let _ = std::fs::remove_file(private_path);
        let _ = std::fs::remove_file(manifest_path);
    }
}
