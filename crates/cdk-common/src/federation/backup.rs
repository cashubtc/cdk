//! Checkpoint-bound federation database backup manifests.
//!
//! The manifest is not a state snapshot or recovery proof. It binds an
//! operator-created database backup to the exact public configuration,
//! externally stored member secrets, durable checkpoint frontier, and active
//! AlephBFT backup that must accompany it.

use bitcoin::hashes::sha256::Hash as Sha256Hash;
use bitcoin::hashes::Hash;
use serde::{Deserialize, Serialize};

use super::{
    append_bytes, FederationCheckpointFrontier, FederationCheckpointId, FederationConfig,
    FederationConfigDigest, FederationError, FederationId, MemberConfigPrivate, MemberId, Result,
    VerifiedFederationCheckpointQuorum,
};

const FEDERATION_SECRET_STORAGE_GENERATION_DOMAIN: &[u8] =
    b"cdk-federation-secret-storage-generation-v1";

/// Backup manifest format version supported by this build.
pub const FEDERATION_DATABASE_BACKUP_MANIFEST_VERSION: u16 = 1;
/// Federation database schema contract required by this backup format.
///
/// This is deliberately independent of a specific SQL backend's migration
/// numbering. It must be bumped whenever persisted federation state changes in
/// a way that makes an older database backup unsafe to restore.
pub const FEDERATION_DATABASE_BACKUP_SCHEMA_VERSION: u32 = 1;

/// Operator-created binding between one database backup and its recovery inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederationDatabaseBackupManifest {
    /// Backup manifest format version.
    pub version: u16,
    /// Federation database schema contract version.
    pub database_schema_version: u32,
    /// Federation whose database was backed up.
    pub federation_id: FederationId,
    /// Digest of the exact public federation configuration.
    pub federation_config_digest: FederationConfigDigest,
    /// Member whose local database and secrets are required.
    pub member_id: MemberId,
    /// Quorum checkpoint trusted by the backed-up database.
    pub trusted_checkpoint_id: FederationCheckpointId,
    /// Exact durable database frontier captured by the backup.
    pub durable_frontier: FederationCheckpointFrontier,
    /// Active AlephBFT session whose local backup must accompany the database.
    pub active_aleph_bft_session: u64,
    /// Number of chunks in the active session backup.
    pub active_backup_chunk_count: u64,
    /// Total bytes in the active session backup.
    pub active_backup_total_bytes: u64,
    /// Domain-separated commitment to the exact external member secret config.
    pub secret_storage_generation: String,
}

impl FederationDatabaseBackupManifest {
    /// Build a validated manifest for one quiesced database backup.
    pub fn new(
        public_config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        trusted_checkpoint: &VerifiedFederationCheckpointQuorum,
        durable_frontier: FederationCheckpointFrontier,
        active_aleph_bft_session: u64,
        active_backup_chunk_count: u64,
        active_backup_total_bytes: u64,
    ) -> Result<Self> {
        public_config.validate()?;
        private_config.validate_against_public(public_config)?;
        durable_frontier.validate_shape()?;
        if !durable_frontier.is_fully_terminal() {
            return Err(backup_manifest_invalid(
                "durable application frontier is not terminal",
            ));
        }
        if durable_frontier.materialized_state_audit_mismatch {
            return Err(backup_manifest_invalid(
                "materialized-state audit mismatch is latched",
            ));
        }

        let checkpoint = trusted_checkpoint.checkpoint();
        let trusted_checkpoint_id = checkpoint.id()?;
        if durable_frontier.trusted_checkpoint_id != Some(trusted_checkpoint_id) {
            return Err(backup_manifest_invalid(
                "durable trusted checkpoint does not match the supplied quorum",
            ));
        }
        durable_frontier.ensure_covers_checkpoint(checkpoint)?;
        if active_aleph_bft_session < checkpoint.aleph_bft_session {
            return Err(backup_manifest_invalid(
                "active AlephBFT session precedes the trusted checkpoint session",
            ));
        }

        let manifest = Self {
            version: FEDERATION_DATABASE_BACKUP_MANIFEST_VERSION,
            database_schema_version: FEDERATION_DATABASE_BACKUP_SCHEMA_VERSION,
            federation_id: public_config.federation_id,
            federation_config_digest: public_config.config_digest_for_validated_config()?,
            member_id: private_config.member_id,
            trusted_checkpoint_id,
            durable_frontier,
            active_aleph_bft_session,
            active_backup_chunk_count,
            active_backup_total_bytes,
            secret_storage_generation: secret_storage_generation(public_config, private_config)?,
        };
        manifest.validate_shape()?;
        Ok(manifest)
    }

    /// Validate format and internal relationships before consulting a database.
    pub fn validate_shape(&self) -> Result<()> {
        if self.version != FEDERATION_DATABASE_BACKUP_MANIFEST_VERSION {
            return Err(backup_manifest_invalid(format!(
                "manifest version mismatch: expected {}, found {}",
                FEDERATION_DATABASE_BACKUP_MANIFEST_VERSION, self.version
            )));
        }
        if self.database_schema_version != FEDERATION_DATABASE_BACKUP_SCHEMA_VERSION {
            return Err(backup_manifest_invalid(format!(
                "database schema version mismatch: expected {}, found {}",
                FEDERATION_DATABASE_BACKUP_SCHEMA_VERSION, self.database_schema_version
            )));
        }
        self.durable_frontier.validate_shape()?;
        if !self.durable_frontier.is_fully_terminal() {
            return Err(backup_manifest_invalid(
                "durable application frontier is not terminal",
            ));
        }
        if self.durable_frontier.materialized_state_audit_mismatch {
            return Err(backup_manifest_invalid(
                "materialized-state audit mismatch is latched",
            ));
        }
        if self.durable_frontier.trusted_checkpoint_id != Some(self.trusted_checkpoint_id) {
            return Err(backup_manifest_invalid(
                "durable frontier is bound to a different trusted checkpoint",
            ));
        }
        let generation = crate::util::hex::decode(&self.secret_storage_generation)
            .map_err(|_| backup_manifest_invalid("secret-storage generation is not valid hex"))?;
        if generation.len() != 32 {
            return Err(backup_manifest_invalid(
                "secret-storage generation must be 32 bytes",
            ));
        }
        Ok(())
    }

    /// Require an operator-supplied manifest to equal current recovery inputs.
    pub fn ensure_matches_current(&self, current: &Self) -> Result<()> {
        self.validate_shape()?;
        current.validate_shape()?;
        ensure_manifest_field(
            self.version == current.version,
            "manifest version does not match this build",
        )?;
        ensure_manifest_field(
            self.database_schema_version == current.database_schema_version,
            "database schema version does not match this build",
        )?;
        ensure_manifest_field(
            self.federation_id == current.federation_id,
            "federation id does not match the restored database",
        )?;
        ensure_manifest_field(
            self.federation_config_digest == current.federation_config_digest,
            "federation config digest does not match the restored database",
        )?;
        ensure_manifest_field(
            self.member_id == current.member_id,
            "member id does not match the restored secret storage",
        )?;
        ensure_manifest_field(
            self.trusted_checkpoint_id == current.trusted_checkpoint_id,
            "trusted checkpoint does not match the restored database",
        )?;
        ensure_manifest_field(
            self.durable_frontier == current.durable_frontier,
            "durable frontier does not match the restored database",
        )?;
        ensure_manifest_field(
            self.active_aleph_bft_session == current.active_aleph_bft_session,
            "active AlephBFT session does not match the restored database",
        )?;
        ensure_manifest_field(
            self.active_backup_chunk_count == current.active_backup_chunk_count
                && self.active_backup_total_bytes == current.active_backup_total_bytes,
            "active AlephBFT backup does not match the restored database",
        )?;
        ensure_manifest_field(
            self.secret_storage_generation == current.secret_storage_generation,
            "secret-storage generation does not match the restored member secrets",
        )
    }
}

fn secret_storage_generation(
    public_config: &FederationConfig,
    private_config: &MemberConfigPrivate,
) -> Result<String> {
    let private_bytes = serde_json::to_vec(private_config)?;
    let mut transcript = Vec::new();
    append_bytes(&mut transcript, FEDERATION_SECRET_STORAGE_GENERATION_DOMAIN);
    append_bytes(&mut transcript, &public_config.federation_id.to_bytes());
    append_bytes(
        &mut transcript,
        &private_config.member_id.get().to_be_bytes(),
    );
    append_bytes(&mut transcript, &private_bytes);
    Ok(crate::util::hex::encode(
        Sha256Hash::hash(&transcript).to_byte_array(),
    ))
}

fn ensure_manifest_field(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(backup_manifest_invalid(reason))
    }
}

fn backup_manifest_invalid(reason: impl Into<String>) -> FederationError {
    FederationError::BackupManifestInvalid {
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::federation::{
        generate_trusted_dealer_configs, FederationCheckpointBoundary, FederationCheckpointQuorum,
        FederationCheckpointSignature, FederationFinalizedItemDigest,
        FederationMaterializedStateDigest, ThresholdParams, TrustedDealerMember,
    };
    use crate::mint_url::MintUrl;
    use crate::SecretKey;

    fn manifest_fixture() -> FederationDatabaseBackupManifest {
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
        .expect("config bundle");
        let private = bundle
            .private_configs
            .get(&MemberId::new(1).expect("member one"))
            .expect("private config");
        let mut durable_frontier = FederationCheckpointFrontier::genesis();
        durable_frontier.finalized_next_item_index = 1;
        durable_frontier.finalized_item_digest = FederationFinalizedItemDigest::from_bytes([6; 32]);
        let boundary = FederationCheckpointBoundary::from_complete_unit(
            &durable_frontier,
            0,
            1,
            bundle
                .public_config
                .members
                .keys()
                .copied()
                .map(|member_id| {
                    (
                        member_id,
                        u64::from(member_id.get() == private.member_id.get()),
                    )
                })
                .collect(),
        )
        .expect("genesis boundary");
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
        let verified = FederationCheckpointQuorum::new(checkpoint.clone(), signatures)
            .verify(&bundle.public_config)
            .expect("verified checkpoint");
        let checkpoint_id = checkpoint.id().expect("checkpoint id");
        let mut frontier = durable_frontier;
        frontier.trusted_checkpoint_id = Some(checkpoint_id);
        frontier.latest_audited_checkpoint_id = Some(checkpoint_id);
        frontier.latest_materialized_state_digest = Some(checkpoint.materialized_state_digest);

        FederationDatabaseBackupManifest::new(
            &bundle.public_config,
            private,
            &verified,
            frontier,
            0,
            2,
            128,
        )
        .expect("backup manifest")
    }

    #[test]
    fn matching_backup_manifest_is_accepted() {
        let manifest = manifest_fixture();
        manifest
            .ensure_matches_current(&manifest)
            .expect("matching manifest");
    }

    #[test]
    fn restore_manifest_rejects_every_bound_generation() {
        let manifest = manifest_fixture();

        let mut wrong = manifest.clone();
        wrong.federation_id = FederationId::from_bytes([8; 32]);
        assert!(manifest.ensure_matches_current(&wrong).is_err());

        let mut wrong = manifest.clone();
        wrong.federation_config_digest = FederationConfigDigest::from_bytes([9; 32]);
        assert!(manifest.ensure_matches_current(&wrong).is_err());

        let mut wrong = manifest.clone();
        wrong.trusted_checkpoint_id = FederationCheckpointId::from_bytes([10; 32]);
        wrong.durable_frontier.trusted_checkpoint_id = Some(wrong.trusted_checkpoint_id);
        assert!(manifest.ensure_matches_current(&wrong).is_err());

        let mut wrong = manifest.clone();
        wrong.database_schema_version += 1;
        assert!(manifest.ensure_matches_current(&wrong).is_err());

        let mut wrong = manifest.clone();
        wrong.secret_storage_generation = crate::util::hex::encode([11; 32]);
        assert!(manifest.ensure_matches_current(&wrong).is_err());
    }

    #[test]
    fn manifest_rejects_nonterminal_or_latched_backups() {
        let manifest = manifest_fixture();

        let mut invalid = manifest.clone();
        invalid.durable_frontier.journal_next_index = 1;
        assert!(invalid.validate_shape().is_err());

        let mut invalid = manifest;
        invalid.durable_frontier.materialized_state_audit_mismatch = true;
        assert!(invalid.validate_shape().is_err());
    }
}
