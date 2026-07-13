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
    status TEXT NOT NULL CHECK (
        status IN ('accepted', 'applying', 'applied', 'rejected', 'failed')
    ),
    CHECK (status = 'rejected' OR rejection_json IS NULL)
);

CREATE INDEX idx_federation_journal_operation_id
ON federation_journal_entries (operation_id);

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
