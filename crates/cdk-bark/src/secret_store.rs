//! Encrypted member-local persistence for incoming Lightning preimages.

use core::fmt;
use std::path::Path;
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use cdk_common::federation::{
    payment_hash_for_preimage, preimage_commitment, FederationBarkActionId, FederationBarkDigest,
    FederationBarkPrepareReceiveSecretRequest, FederationBarkReceiveSecret,
    FederationBarkReceiveSecretAcknowledgement, FederationBarkReceiveSecretBinding,
    FederationCheckpointId, FederationConfig, FederationId, FederationOperationId,
    MemberConfigPrivate, MemberId, VerifiedFederationCheckpointQuorum,
};
use cdk_frost::rand_core::{CryptoRng, OsRng, RngCore};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::Error;

/// Earliest local deletion of an unaccepted protected-secret orphan.
pub const FEDERATED_BARK_RECEIVE_SECRET_ORPHAN_RETENTION_SECONDS: u64 = 24 * 60 * 60;
/// Minimum post-checkpoint retention accepted by the v1 store.
pub const FEDERATED_BARK_RECEIVE_SECRET_MINIMUM_RETENTION_DAYS: u16 = 30;

const STORE_SCHEMA_VERSION: u16 = 1;
const BACKUP_VERSION: u16 = 1;
const SEALED_MAGIC: &[u8; 8] = b"CDKBRS01";
const NONCE_LEN: usize = 12;
const MAX_CIPHERTEXT_BYTES: usize = 128;
const MAX_BACKUP_BYTES: usize = 16 * 1024 * 1024;
const MAX_BACKUP_RECORDS: usize = 4096;
const ENCRYPTION_AAD_DOMAIN: &[u8] = b"cdk/bark/receive-secret-encryption/v1";
const RECORDS: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("federated_bark_receive_secrets_v1");

/// Independent at-rest encryption key for protected receive secrets.
pub struct FederatedBarkReceiveSecretEncryptionKey(Zeroizing<[u8; 32]>);

impl FederatedBarkReceiveSecretEncryptionKey {
    /// Import an operator-provided non-zero 256-bit storage key.
    pub fn new(bytes: [u8; 32]) -> Result<Self, Error> {
        if bytes == [0; 32] {
            return Err(Error::InvalidReceiveSecretEncryptionKey);
        }
        Ok(Self(Zeroizing::new(bytes)))
    }
}

impl fmt::Debug for FederatedBarkReceiveSecretEncryptionKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FederatedBarkReceiveSecretEncryptionKey(<redacted>)")
    }
}

/// Durable accepted-intent reference retained with a protected secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedBarkReceiveSecretAcceptedRef {
    /// Accepted receive-intent operation.
    pub operation_id: FederationOperationId,
    /// Consensus journal index of the accepted intent.
    pub operation_index: u64,
}

/// Terminal and checkpoint evidence required before retention can expire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FederatedBarkReceiveSecretTerminalRef {
    /// Terminal accepted operation.
    pub operation_id: FederationOperationId,
    /// Consensus journal index of the terminal operation.
    pub operation_index: u64,
    /// Quorum-certified checkpoint covering the terminal operation.
    pub checkpoint_id: FederationCheckpointId,
    /// Member-local time when checkpoint coverage was durably recorded.
    pub checkpointed_at: u64,
}

/// Redacted lifecycle metadata for one protected receive secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FederatedBarkReceiveSecretStatus {
    /// Stable action identifier.
    pub action_id: FederationBarkActionId,
    /// Exact public record commitment.
    pub record_hash: FederationBarkDigest,
    /// Local receipt time.
    pub received_at: u64,
    /// Whether the local durable acknowledgement is present.
    pub acknowledged: bool,
    /// Accepted intent, when consensus has retained the record.
    pub accepted: Option<FederatedBarkReceiveSecretAcceptedRef>,
    /// Terminal checkpoint evidence, when deletion may eventually become eligible.
    pub terminal: Option<FederatedBarkReceiveSecretTerminalRef>,
}

/// Decrypted owner of one exact resumable receive record.
pub struct FederatedBarkResumableReceiveSecret {
    /// Complete public binding used to reconstruct a receive intent.
    pub binding: FederationBarkReceiveSecretBinding,
    /// Durable local acknowledgement, when signing completed.
    pub acknowledgement: Option<FederationBarkReceiveSecretAcknowledgement>,
    /// Accepted intent reference, when already known locally.
    pub accepted: Option<FederatedBarkReceiveSecretAcceptedRef>,
    /// Zeroizing protected preimage owner.
    pub secret: FederationBarkReceiveSecret,
}

impl fmt::Debug for FederatedBarkResumableReceiveSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkResumableReceiveSecret")
            .field("binding", &self.binding)
            .field("acknowledgement", &self.acknowledgement)
            .field("accepted", &self.accepted)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredReceiveSecret {
    version: u16,
    binding: FederationBarkReceiveSecretBinding,
    record_hash: FederationBarkDigest,
    ciphertext: Vec<u8>,
    received_at: u64,
    acknowledgement: Option<FederationBarkReceiveSecretAcknowledgement>,
    accepted: Option<FederatedBarkReceiveSecretAcceptedRef>,
    terminal: Option<FederatedBarkReceiveSecretTerminalRef>,
}

impl fmt::Debug for StoredReceiveSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredReceiveSecret")
            .field("version", &self.version)
            .field("binding", &self.binding)
            .field("record_hash", &self.record_hash)
            .field("ciphertext", &"<redacted>")
            .field("received_at", &self.received_at)
            .field("acknowledgement", &self.acknowledgement)
            .field("accepted", &self.accepted)
            .field("terminal", &self.terminal)
            .finish()
    }
}

impl StoredReceiveSecret {
    fn status(&self) -> FederatedBarkReceiveSecretStatus {
        FederatedBarkReceiveSecretStatus {
            action_id: self.binding.action_id,
            record_hash: self.record_hash,
            received_at: self.received_at,
            acknowledged: self.acknowledgement.is_some(),
            accepted: self.accepted,
            terminal: self.terminal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiveSecretBackup {
    version: u16,
    records: Vec<StoredReceiveSecret>,
}

/// Transactional encrypted receive-secret store.
pub struct FederatedBarkReceiveSecretStore {
    database: Database,
    encryption_key: Arc<FederatedBarkReceiveSecretEncryptionKey>,
}

impl fmt::Debug for FederatedBarkReceiveSecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FederatedBarkReceiveSecretStore")
            .field("database", &"<member-local>")
            .field("encryption_key", &"<redacted>")
            .finish()
    }
}

impl FederatedBarkReceiveSecretStore {
    /// Open or initialize the application-scoped store without changing Bark SQLite.
    pub fn open(
        path: impl AsRef<Path>,
        encryption_key: Arc<FederatedBarkReceiveSecretEncryptionKey>,
    ) -> Result<Self, Error> {
        let database = Database::create(path).map_err(|_| Error::ReceiveSecretStoreOpenFailed)?;
        let transaction = database
            .begin_write()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        {
            let _ = transaction
                .open_table(RECORDS)
                .map_err(|_| Error::ReceiveSecretPersistence)?;
        }
        transaction
            .commit()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        Ok(Self {
            database,
            encryption_key,
        })
    }

    /// Persist, then sign, one exact authenticated prepare-secret request.
    ///
    /// The first transaction commits encrypted secret material before the
    /// acknowledgement is constructed. A crash between those steps leaves a
    /// recoverable unacknowledged record; an exact retry deterministically
    /// completes it.
    pub fn prepare_receive_secret(
        &self,
        request: &FederationBarkPrepareReceiveSecretRequest,
        authenticated_sender: MemberId,
        config: &FederationConfig,
        private_config: &MemberConfigPrivate,
        received_at: u64,
    ) -> Result<FederationBarkReceiveSecretAcknowledgement, Error> {
        private_config
            .validate_against_public(config)
            .map_err(|_| Error::ReceiveSecretRequestInvalid)?;
        request
            .validate(config, authenticated_sender, private_config.member_id)
            .map_err(|_| Error::ReceiveSecretRequestInvalid)?;
        let existing_acknowledgement = self.persist_before_ack(request, received_at, &mut OsRng)?;
        match existing_acknowledgement {
            Some(acknowledgement) => {
                acknowledgement
                    .verify(config, &request.binding)
                    .map_err(|_| Error::ReceiveSecretRecordInvalid)?;
                Ok(acknowledgement)
            }
            None => {
                let acknowledgement = FederationBarkReceiveSecretAcknowledgement::new_signed(
                    &request.binding,
                    private_config,
                )
                .map_err(|_| Error::ReceiveSecretAcknowledgementFailed)?;
                self.persist_acknowledgement(request.binding.action_id, acknowledgement)?;
                Ok(acknowledgement)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn persist_before_ack_for_test<R>(
        &self,
        request: &FederationBarkPrepareReceiveSecretRequest,
        received_at: u64,
        rng: &mut R,
    ) -> Result<Option<FederationBarkReceiveSecretAcknowledgement>, Error>
    where
        R: CryptoRng + RngCore,
    {
        self.persist_before_ack(request, received_at, rng)
    }

    /// Load and authenticate an encrypted preimage for an exact binding.
    pub fn load_secret(
        &self,
        binding: &FederationBarkReceiveSecretBinding,
    ) -> Result<FederationBarkReceiveSecret, Error> {
        let record = self
            .load_record(binding.action_id)?
            .ok_or(Error::ReceiveSecretUnavailable)?;
        if &record.binding != binding
            || record.record_hash
                != binding
                    .record_hash()
                    .map_err(|_| Error::ReceiveSecretRecordInvalid)?
        {
            return Err(Error::ReceiveSecretConflict);
        }
        self.decrypt_record(&record)
    }

    /// Find the sole protected receive record for a preallocated quote.
    ///
    /// This bounded scan is used only at the HTTP orchestration boundary to
    /// resume a crash between local secret persistence and intent acceptance.
    /// Multiple action bindings for one quote fail closed.
    pub fn find_by_quote(
        &self,
        federation_id: FederationId,
        quote: &str,
    ) -> Result<Option<FederatedBarkResumableReceiveSecret>, Error> {
        let transaction = self
            .database
            .begin_read()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        let table = transaction
            .open_table(RECORDS)
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        let mut found = None;
        for row in table.iter().map_err(|_| Error::ReceiveSecretPersistence)? {
            let (_, value) = row.map_err(|_| Error::ReceiveSecretPersistence)?;
            let record = decode_record(value.value())?;
            if record.binding.federation_id != federation_id || record.binding.quote != quote {
                continue;
            }
            if found.is_some() {
                return Err(Error::ReceiveSecretConflict);
            }
            let secret = self.decrypt_record(&record)?;
            found = Some(FederatedBarkResumableReceiveSecret {
                binding: record.binding,
                acknowledgement: record.acknowledgement,
                accepted: record.accepted,
                secret,
            });
        }
        Ok(found)
    }

    /// Return redacted lifecycle status without decrypting secret material.
    pub fn status(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Option<FederatedBarkReceiveSecretStatus>, Error> {
        let Some(record) = self.load_record(action_id)? else {
            return Ok(None);
        };
        let _ = self.decrypt_record(&record)?;
        Ok(Some(record.status()))
    }

    /// Mark the exact intent accepted so orphan cleanup can never delete it.
    pub fn mark_intent_accepted(
        &self,
        action_id: FederationBarkActionId,
        accepted: FederatedBarkReceiveSecretAcceptedRef,
    ) -> Result<(), Error> {
        self.update_record(action_id, |record| match record.accepted {
            Some(existing) if existing == accepted => Ok(false),
            Some(_) => Err(Error::ReceiveSecretConflict),
            None => {
                record.accepted = Some(accepted);
                Ok(true)
            }
        })
    }

    /// Record terminal accepted state plus a verified checkpoint that covers it.
    pub fn mark_terminal_checkpointed(
        &self,
        action_id: FederationBarkActionId,
        operation_id: FederationOperationId,
        operation_index: u64,
        checkpoint: &VerifiedFederationCheckpointQuorum,
        checkpointed_at: u64,
    ) -> Result<(), Error> {
        self.update_record(action_id, |record| {
            let accepted = record
                .accepted
                .ok_or(Error::ReceiveSecretLifecycleInvalid)?;
            let checkpoint = checkpoint.checkpoint();
            if operation_index < accepted.operation_index
                || checkpoint.federation_id != record.binding.federation_id
                || checkpoint.journal_next_index <= operation_index
            {
                return Err(Error::ReceiveSecretLifecycleInvalid);
            }
            let terminal = FederatedBarkReceiveSecretTerminalRef {
                operation_id,
                operation_index,
                checkpoint_id: checkpoint
                    .id()
                    .map_err(|_| Error::ReceiveSecretLifecycleInvalid)?,
                checkpointed_at,
            };
            match record.terminal {
                Some(existing) if existing == terminal => Ok(false),
                Some(_) => Err(Error::ReceiveSecretConflict),
                None => {
                    record.terminal = Some(terminal);
                    Ok(true)
                }
            }
        })
    }

    /// Delete only expired orphans and checkpoint-covered terminal records.
    pub fn cleanup(&self, now: u64, accepted_retention_days: u16) -> Result<usize, Error> {
        if accepted_retention_days < FEDERATED_BARK_RECEIVE_SECRET_MINIMUM_RETENTION_DAYS {
            return Err(Error::ReceiveSecretRetentionInvalid);
        }
        let accepted_retention_seconds = u64::from(accepted_retention_days)
            .checked_mul(24 * 60 * 60)
            .ok_or(Error::ReceiveSecretRetentionInvalid)?;
        let transaction = self
            .database
            .begin_write()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        let mut removed = 0usize;
        {
            let mut table = transaction
                .open_table(RECORDS)
                .map_err(|_| Error::ReceiveSecretPersistence)?;
            let mut candidates = Vec::new();
            for row in table.iter().map_err(|_| Error::ReceiveSecretPersistence)? {
                let (key, value) = row.map_err(|_| Error::ReceiveSecretPersistence)?;
                let record = decode_record(value.value())?;
                let _ = self.decrypt_record(&record)?;
                let delete = match (record.accepted, record.terminal) {
                    (None, _) => {
                        now >= record
                            .received_at
                            .saturating_add(FEDERATED_BARK_RECEIVE_SECRET_ORPHAN_RETENTION_SECONDS)
                    }
                    (Some(_), Some(terminal)) => {
                        now >= terminal
                            .checkpointed_at
                            .saturating_add(accepted_retention_seconds)
                    }
                    (Some(_), None) => false,
                };
                if delete {
                    candidates.push(key.value().to_vec());
                }
            }
            for key in candidates {
                if table
                    .remove(key.as_slice())
                    .map_err(|_| Error::ReceiveSecretPersistence)?
                    .is_some()
                {
                    removed = removed.saturating_add(1);
                }
            }
        }
        transaction
            .commit()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        Ok(removed)
    }

    /// Export a bounded encrypted backup containing no plaintext or storage key.
    pub fn export_backup(&self) -> Result<Vec<u8>, Error> {
        let transaction = self
            .database
            .begin_read()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        let table = transaction
            .open_table(RECORDS)
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        let mut records = Vec::new();
        for row in table.iter().map_err(|_| Error::ReceiveSecretPersistence)? {
            let (_, value) = row.map_err(|_| Error::ReceiveSecretPersistence)?;
            let record = decode_record(value.value())?;
            let _ = self.decrypt_record(&record)?;
            records.push(record);
        }
        if records.len() > MAX_BACKUP_RECORDS {
            return Err(Error::ReceiveSecretBackupInvalid);
        }
        let bytes = serde_json::to_vec(&ReceiveSecretBackup {
            version: BACKUP_VERSION,
            records,
        })
        .map_err(|_| Error::ReceiveSecretBackupInvalid)?;
        if bytes.len() > MAX_BACKUP_BYTES {
            return Err(Error::ReceiveSecretBackupInvalid);
        }
        Ok(bytes)
    }

    /// Import an encrypted backup idempotently after full binding and decryption checks.
    pub fn import_backup(&self, bytes: &[u8], config: &FederationConfig) -> Result<(), Error> {
        if bytes.len() > MAX_BACKUP_BYTES {
            return Err(Error::ReceiveSecretBackupInvalid);
        }
        let backup: ReceiveSecretBackup =
            serde_json::from_slice(bytes).map_err(|_| Error::ReceiveSecretBackupInvalid)?;
        if backup.version != BACKUP_VERSION || backup.records.len() > MAX_BACKUP_RECORDS {
            return Err(Error::ReceiveSecretBackupInvalid);
        }
        for record in &backup.records {
            self.validate_record(record, config)?;
        }
        let transaction = self
            .database
            .begin_write()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        {
            let mut table = transaction
                .open_table(RECORDS)
                .map_err(|_| Error::ReceiveSecretPersistence)?;
            for record in &backup.records {
                let key = record.binding.action_id.to_bytes();
                let existing = table
                    .get(key.as_slice())
                    .map_err(|_| Error::ReceiveSecretPersistence)?
                    .map(|value| value.value().to_vec());
                match existing {
                    Some(existing) if decode_record(&existing)? == *record => {}
                    Some(_) => return Err(Error::ReceiveSecretConflict),
                    None => {
                        let encoded = encode_record(record)?;
                        table
                            .insert(key.as_slice(), encoded.as_slice())
                            .map_err(|_| Error::ReceiveSecretPersistence)?;
                    }
                }
            }
        }
        transaction
            .commit()
            .map_err(|_| Error::ReceiveSecretPersistence)
    }

    fn persist_before_ack<R>(
        &self,
        request: &FederationBarkPrepareReceiveSecretRequest,
        received_at: u64,
        rng: &mut R,
    ) -> Result<Option<FederationBarkReceiveSecretAcknowledgement>, Error>
    where
        R: CryptoRng + RngCore,
    {
        let record_hash = request
            .binding
            .record_hash()
            .map_err(|_| Error::ReceiveSecretRecordInvalid)?;
        let key = request.binding.action_id.to_bytes();
        let transaction = self
            .database
            .begin_write()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        let existing_acknowledgement;
        {
            let mut table = transaction
                .open_table(RECORDS)
                .map_err(|_| Error::ReceiveSecretPersistence)?;
            let existing = table
                .get(key.as_slice())
                .map_err(|_| Error::ReceiveSecretPersistence)?
                .map(|value| value.value().to_vec());
            match existing {
                Some(existing) => {
                    let record = decode_record(&existing)?;
                    if record.binding != request.binding || record.record_hash != record_hash {
                        return Err(Error::ReceiveSecretConflict);
                    }
                    let secret = self.decrypt_record(&record)?;
                    if secret.as_bytes() != request.secret().as_bytes() {
                        return Err(Error::ReceiveSecretConflict);
                    }
                    existing_acknowledgement = record.acknowledgement;
                }
                None => {
                    let mut record = StoredReceiveSecret {
                        version: STORE_SCHEMA_VERSION,
                        binding: request.binding.clone(),
                        record_hash,
                        ciphertext: Vec::new(),
                        received_at,
                        acknowledgement: None,
                        accepted: None,
                        terminal: None,
                    };
                    record.ciphertext = seal(
                        &self.encryption_key,
                        &encryption_aad(&record)?,
                        request.secret().as_bytes(),
                        rng,
                    )?;
                    let encoded = encode_record(&record)?;
                    table
                        .insert(key.as_slice(), encoded.as_slice())
                        .map_err(|_| Error::ReceiveSecretPersistence)?;
                    existing_acknowledgement = None;
                }
            }
        }
        transaction
            .commit()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        Ok(existing_acknowledgement)
    }

    fn persist_acknowledgement(
        &self,
        action_id: FederationBarkActionId,
        acknowledgement: FederationBarkReceiveSecretAcknowledgement,
    ) -> Result<(), Error> {
        self.update_record(action_id, |record| match record.acknowledgement {
            Some(existing) if existing == acknowledgement => Ok(false),
            Some(_) => Err(Error::ReceiveSecretConflict),
            None => {
                record.acknowledgement = Some(acknowledgement);
                Ok(true)
            }
        })
    }

    fn update_record<F>(&self, action_id: FederationBarkActionId, update: F) -> Result<(), Error>
    where
        F: FnOnce(&mut StoredReceiveSecret) -> Result<bool, Error>,
    {
        let key = action_id.to_bytes();
        let transaction = self
            .database
            .begin_write()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        {
            let mut table = transaction
                .open_table(RECORDS)
                .map_err(|_| Error::ReceiveSecretPersistence)?;
            let mut record = table
                .get(key.as_slice())
                .map_err(|_| Error::ReceiveSecretPersistence)?
                .map(|value| decode_record(value.value()))
                .transpose()?
                .ok_or(Error::ReceiveSecretUnavailable)?;
            let secret = self.decrypt_record(&record)?;
            if update(&mut record)? {
                record.ciphertext = seal(
                    &self.encryption_key,
                    &encryption_aad(&record)?,
                    secret.as_bytes(),
                    &mut OsRng,
                )?;
                let encoded = encode_record(&record)?;
                table
                    .insert(key.as_slice(), encoded.as_slice())
                    .map_err(|_| Error::ReceiveSecretPersistence)?;
            }
        }
        transaction
            .commit()
            .map_err(|_| Error::ReceiveSecretPersistence)
    }

    fn load_record(
        &self,
        action_id: FederationBarkActionId,
    ) -> Result<Option<StoredReceiveSecret>, Error> {
        let key = action_id.to_bytes();
        let transaction = self
            .database
            .begin_read()
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        let table = transaction
            .open_table(RECORDS)
            .map_err(|_| Error::ReceiveSecretPersistence)?;
        table
            .get(key.as_slice())
            .map_err(|_| Error::ReceiveSecretPersistence)?
            .map(|value| decode_record(value.value()))
            .transpose()
    }

    fn decrypt_record(
        &self,
        record: &StoredReceiveSecret,
    ) -> Result<FederationBarkReceiveSecret, Error> {
        let plaintext = open(
            &self.encryption_key,
            &encryption_aad(record)?,
            &record.ciphertext,
        )?;
        let bytes: [u8; 32] = plaintext
            .as_slice()
            .try_into()
            .map_err(|_| Error::ReceiveSecretRecordInvalid)?;
        if payment_hash_for_preimage(&bytes).map_err(|_| Error::ReceiveSecretRecordInvalid)?
            != record.binding.payment_hash
            || preimage_commitment(&bytes).map_err(|_| Error::ReceiveSecretRecordInvalid)?
                != record.binding.preimage_commitment
        {
            return Err(Error::ReceiveSecretRecordInvalid);
        }
        Ok(FederationBarkReceiveSecret::from_bytes(bytes))
    }

    fn validate_record(
        &self,
        record: &StoredReceiveSecret,
        config: &FederationConfig,
    ) -> Result<(), Error> {
        if record.version != STORE_SCHEMA_VERSION
            || record.record_hash
                != record
                    .binding
                    .record_hash()
                    .map_err(|_| Error::ReceiveSecretRecordInvalid)?
        {
            return Err(Error::ReceiveSecretRecordInvalid);
        }
        record
            .binding
            .validate(config)
            .map_err(|_| Error::ReceiveSecretRecordInvalid)?;
        if let Some(acknowledgement) = record.acknowledgement {
            acknowledgement
                .verify(config, &record.binding)
                .map_err(|_| Error::ReceiveSecretRecordInvalid)?;
        }
        let _ = self.decrypt_record(record)?;
        Ok(())
    }
}

fn encode_record(record: &StoredReceiveSecret) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(record).map_err(|_| Error::ReceiveSecretPersistence)
}

fn decode_record(bytes: &[u8]) -> Result<StoredReceiveSecret, Error> {
    let record: StoredReceiveSecret =
        serde_json::from_slice(bytes).map_err(|_| Error::ReceiveSecretRecordInvalid)?;
    if record.version != STORE_SCHEMA_VERSION || record.ciphertext.len() > MAX_CIPHERTEXT_BYTES {
        return Err(Error::ReceiveSecretRecordInvalid);
    }
    Ok(record)
}

#[derive(Serialize)]
struct ReceiveSecretEncryptionAad<'a> {
    version: u16,
    binding: &'a FederationBarkReceiveSecretBinding,
    record_hash: FederationBarkDigest,
    received_at: u64,
    acknowledgement: Option<FederationBarkReceiveSecretAcknowledgement>,
    accepted: Option<FederatedBarkReceiveSecretAcceptedRef>,
    terminal: Option<FederatedBarkReceiveSecretTerminalRef>,
}

fn encryption_aad(record: &StoredReceiveSecret) -> Result<Vec<u8>, Error> {
    let mut aad = Vec::new();
    append_bytes(&mut aad, ENCRYPTION_AAD_DOMAIN);
    let metadata = serde_json::to_vec(&ReceiveSecretEncryptionAad {
        version: record.version,
        binding: &record.binding,
        record_hash: record.record_hash,
        received_at: record.received_at,
        acknowledgement: record.acknowledgement,
        accepted: record.accepted,
        terminal: record.terminal,
    })
    .map_err(|_| Error::ReceiveSecretRecordInvalid)?;
    append_bytes(&mut aad, &metadata);
    Ok(aad)
}

fn seal<R>(
    key: &FederatedBarkReceiveSecretEncryptionKey,
    aad: &[u8],
    plaintext: &[u8],
    rng: &mut R,
) -> Result<Vec<u8>, Error>
where
    R: CryptoRng + RngCore,
{
    let mut nonce = [0; NONCE_LEN];
    rng.fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(key.0.as_ref())
        .map_err(|_| Error::ReceiveSecretEncryptionFailed)?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| Error::ReceiveSecretEncryptionFailed)?;
    let mut encoded = Vec::with_capacity(SEALED_MAGIC.len() + NONCE_LEN + ciphertext.len());
    encoded.extend_from_slice(SEALED_MAGIC);
    encoded.extend_from_slice(&nonce);
    encoded.extend_from_slice(&ciphertext);
    Ok(encoded)
}

fn open(
    key: &FederatedBarkReceiveSecretEncryptionKey,
    aad: &[u8],
    encrypted: &[u8],
) -> Result<Zeroizing<Vec<u8>>, Error> {
    let ciphertext_start = SEALED_MAGIC.len() + NONCE_LEN;
    if encrypted.len() < ciphertext_start + 16
        || encrypted.len() > MAX_CIPHERTEXT_BYTES
        || &encrypted[..SEALED_MAGIC.len()] != SEALED_MAGIC
    {
        return Err(Error::ReceiveSecretRecordInvalid);
    }
    let cipher = Aes256Gcm::new_from_slice(key.0.as_ref())
        .map_err(|_| Error::ReceiveSecretDecryptionFailed)?;
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(&encrypted[SEALED_MAGIC.len()..ciphertext_start]),
            Payload {
                msg: &encrypted[ciphertext_start..],
                aad,
            },
        )
        .map_err(|_| Error::ReceiveSecretDecryptionFailed)?;
    Ok(Zeroizing::new(plaintext))
}

fn append_bytes(transcript: &mut Vec<u8>, value: &[u8]) {
    transcript.extend_from_slice(&(value.len() as u32).to_be_bytes());
    transcript.extend_from_slice(value);
}
