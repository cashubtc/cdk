ALTER TABLE saga_state ADD COLUMN federation_operation_id TEXT;

CREATE TABLE federation_journal_entries (
    operation_index BIGINT PRIMARY KEY,
    operation_id TEXT NOT NULL UNIQUE,
    envelope_json TEXT NOT NULL,
    conflict_keys_json TEXT NOT NULL,
    previous_order_digest TEXT NOT NULL,
    order_digest TEXT NOT NULL,
    consensus_evidence_json TEXT,
    rejection_json TEXT,
    application_digest TEXT,
    status TEXT NOT NULL CHECK (
        status IN ('accepted', 'applying', 'applied', 'rejected', 'failed')
    ),
    CHECK (status = 'rejected' OR rejection_json IS NULL)
);

CREATE INDEX idx_federation_journal_operation_id
ON federation_journal_entries (operation_id);

CREATE TABLE federation_journal_conflict_keys (
    conflict_key_json TEXT PRIMARY KEY,
    operation_id TEXT NOT NULL REFERENCES federation_journal_entries(operation_id)
);

CREATE TABLE federation_dkg_member_sequences (
    member_id INTEGER PRIMARY KEY,
    last_sequence BIGINT NOT NULL
);

CREATE TABLE federation_catch_up_certificates (
    from_index BIGINT NOT NULL,
    signer_member_id INTEGER NOT NULL,
    certificate_json TEXT NOT NULL,
    PRIMARY KEY (from_index, signer_member_id)
);

CREATE INDEX idx_federation_catch_up_certificates_from_index
ON federation_catch_up_certificates (from_index);

CREATE TABLE federation_checkpoints (
    checkpoint_id TEXT PRIMARY KEY NOT NULL,
    checkpoint_kind TEXT NOT NULL CHECK (
        checkpoint_kind IN ('scheduled', 'recovery')
    ),
    scheduled_checkpoint_sequence BIGINT NOT NULL,
    scheduled_anchor_id TEXT,
    journal_next_index BIGINT NOT NULL,
    finalized_next_item_index BIGINT NOT NULL,
    checkpoint_json TEXT NOT NULL
);

CREATE INDEX idx_federation_checkpoints_kind_sequence
ON federation_checkpoints (
    checkpoint_kind,
    scheduled_checkpoint_sequence,
    finalized_next_item_index,
    journal_next_index
);

CREATE INDEX idx_federation_checkpoints_finalized_cursor
ON federation_checkpoints (finalized_next_item_index);

CREATE TABLE federation_checkpoint_signatures (
    checkpoint_id TEXT NOT NULL,
    signer_member_id INTEGER NOT NULL,
    signature_json TEXT NOT NULL,
    PRIMARY KEY (checkpoint_id, signer_member_id)
);

CREATE INDEX idx_federation_checkpoint_signatures_checkpoint
ON federation_checkpoint_signatures (checkpoint_id);

CREATE TABLE federation_checkpoint_quorums (
    checkpoint_id TEXT PRIMARY KEY NOT NULL,
    quorum_json TEXT NOT NULL
);

CREATE TABLE federation_checkpoint_state (
    state_key TEXT PRIMARY KEY NOT NULL CHECK (state_key = 'active'),
    journal_next_index BIGINT NOT NULL,
    journal_order_digest TEXT NOT NULL,
    finalized_next_item_index BIGINT NOT NULL,
    finalized_item_digest TEXT NOT NULL,
    application_next_index BIGINT NOT NULL,
    application_digest TEXT NOT NULL,
    trusted_checkpoint_id TEXT,
    latest_audited_checkpoint_id TEXT,
    latest_materialized_state_digest TEXT,
    materialized_state_audit_mismatch INTEGER NOT NULL DEFAULT 0 CHECK (
        materialized_state_audit_mismatch IN (0, 1)
    )
);

CREATE TABLE federation_materialized_state_keys (
    state_kind TEXT NOT NULL,
    state_key TEXT NOT NULL,
    operation_index BIGINT NOT NULL,
    operation_id TEXT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (state_kind, state_key)
);

CREATE INDEX idx_federation_materialized_state_keys_operation
ON federation_materialized_state_keys (operation_index, operation_id);

CREATE TABLE federation_payment_observations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_index BIGINT NOT NULL,
    context TEXT NOT NULL,
    quote TEXT NOT NULL,
    observer_member_id INTEGER NOT NULL,
    state TEXT NOT NULL,
    observation_json TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_federation_payment_observations_member_state
ON federation_payment_observations (context, quote, observer_member_id, state);

CREATE INDEX idx_federation_payment_observations_quote
ON federation_payment_observations (context, quote, operation_index);

CREATE TABLE federation_signature_shares (
    operation_id TEXT NOT NULL,
    output_index INTEGER NOT NULL,
    member_id INTEGER NOT NULL,
    blinded_message BYTEA NOT NULL,
    amount INTEGER NOT NULL,
    keyset_id TEXT NOT NULL,
    signature_share BYTEA NOT NULL,
    share_json TEXT NOT NULL,
    PRIMARY KEY (operation_id, output_index, member_id)
);

CREATE INDEX idx_federation_signature_shares_operation_output
ON federation_signature_shares (operation_id, output_index);

CREATE INDEX idx_federation_signature_shares_blinded_message
ON federation_signature_shares (blinded_message);

CREATE TABLE federation_config (
    federation_id TEXT PRIMARY KEY NOT NULL,
    config_digest TEXT NOT NULL,
    config_json TEXT NOT NULL
);

CREATE TABLE federation_aleph_bft_finalized_items (
    accepted_item_index BIGINT PRIMARY KEY NOT NULL,
    session BIGINT NOT NULL,
    unit_finalization_index BIGINT NOT NULL,
    unit_submitting_member INTEGER NOT NULL,
    unit_index BIGINT NOT NULL,
    item_index_in_unit BIGINT NOT NULL,
    operation_id TEXT,
    status TEXT NOT NULL,
    unit_body_hash TEXT NOT NULL,
    finalized_item_digest TEXT,
    item_json TEXT NOT NULL
);

CREATE INDEX idx_federation_aleph_bft_finalized_items_session_unit
ON federation_aleph_bft_finalized_items (
    session,
    unit_submitting_member,
    unit_index
);

CREATE INDEX idx_federation_aleph_bft_finalized_items_operation_id
ON federation_aleph_bft_finalized_items (operation_id);

CREATE TABLE federation_aleph_bft_backups (
    session BIGINT PRIMARY KEY NOT NULL,
    backup_bytes BYTEA NOT NULL
);

CREATE TABLE federation_aleph_bft_pending_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    session BIGINT NOT NULL,
    envelope_json TEXT NOT NULL
);

CREATE INDEX idx_federation_aleph_bft_pending_operations_session
ON federation_aleph_bft_pending_operations (session);

CREATE TABLE federation_aleph_bft_session_state (
    session_key TEXT PRIMARY KEY,
    active_session BIGINT NOT NULL CHECK (active_session >= 0)
);

CREATE TABLE federation_dkg_setup_messages (
    ceremony_id TEXT NOT NULL,
    phase TEXT NOT NULL,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (ceremony_id, phase, member_id)
);

CREATE INDEX idx_federation_dkg_setup_member
ON federation_dkg_setup_messages (ceremony_id, member_id);

CREATE TABLE federation_dkg_participant_states (
    ceremony_id TEXT PRIMARY KEY,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE INDEX idx_federation_dkg_participant_states_member
ON federation_dkg_participant_states (member_id);

CREATE TABLE federation_dkg_member_keyset_shares (
    keyset_id TEXT PRIMARY KEY,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);

CREATE INDEX idx_federation_dkg_member_keyset_shares_member
ON federation_dkg_member_keyset_shares (member_id);

CREATE TABLE federation_dkg_secret_share_contributions (
    ceremony_id TEXT NOT NULL,
    sender_id INTEGER NOT NULL,
    receiver_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (ceremony_id, sender_id, receiver_id)
);

CREATE INDEX idx_federation_dkg_secret_share_contributions_receiver
ON federation_dkg_secret_share_contributions (ceremony_id, receiver_id);

CREATE TABLE federation_dkg_transcript_signatures (
    transcript_hash TEXT NOT NULL,
    member_id INTEGER NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (transcript_hash, member_id)
);

CREATE INDEX idx_federation_dkg_transcript_signatures_member
ON federation_dkg_transcript_signatures (member_id);

CREATE TABLE federation_dkg_activations (
    ceremony_id TEXT NOT NULL,
    member_id BIGINT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (ceremony_id, member_id)
);

CREATE INDEX idx_federation_dkg_activations_member
ON federation_dkg_activations (member_id);

CREATE TABLE federation_dkg_ceremonies (
    ceremony_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL
);

CREATE TABLE federation_frost_dkg_records (
    ceremony_id TEXT NOT NULL,
    record_kind TEXT NOT NULL,
    sender_id BIGINT NOT NULL,
    receiver_id BIGINT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (ceremony_id, record_kind, sender_id, receiver_id)
);

CREATE INDEX idx_federation_frost_dkg_records_kind
ON federation_frost_dkg_records (record_kind, ceremony_id, sender_id, receiver_id);

CREATE TABLE federation_frost_wallet_config (
    record_kind TEXT NOT NULL,
    root_key_epoch BIGINT NOT NULL,
    application_id TEXT NOT NULL,
    application_key_epoch BIGINT NOT NULL,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (record_kind, root_key_epoch, application_id, application_key_epoch)
);

CREATE INDEX idx_federation_frost_wallet_config_kind
ON federation_frost_wallet_config (record_kind, root_key_epoch, application_id, application_key_epoch);
