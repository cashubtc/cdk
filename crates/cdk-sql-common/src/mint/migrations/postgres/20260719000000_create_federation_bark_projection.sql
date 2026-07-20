CREATE TABLE federation_bark_actions (
    action_id TEXT PRIMARY KEY NOT NULL,
    quote TEXT UNIQUE,
    payment_hash TEXT UNIQUE,
    action_kind TEXT,
    phase TEXT NOT NULL,
    actor_member_id BIGINT NOT NULL,
    attempt BIGINT NOT NULL,
    origin_operation_id TEXT UNIQUE NOT NULL,
    origin_operation_index BIGINT NOT NULL,
    latest_operation_id TEXT UNIQUE NOT NULL,
    latest_operation_index BIGINT UNIQUE NOT NULL,
    plan_operation_id TEXT,
    predecessor_operation_id TEXT,
    operation_commitment TEXT NOT NULL,
    record_json TEXT NOT NULL
);

CREATE INDEX idx_federation_bark_actions_phase
ON federation_bark_actions (phase, latest_operation_index);

CREATE TABLE federation_bark_vtxos (
    vtxo_id TEXT PRIMARY KEY NOT NULL,
    locator_role BIGINT NOT NULL,
    locator_index BIGINT NOT NULL,
    locator_version BIGINT NOT NULL,
    amount BIGINT NOT NULL,
    state TEXT NOT NULL,
    artifact_hash TEXT NOT NULL,
    reservation_action_id TEXT,
    origin_operation_id TEXT NOT NULL,
    origin_operation_index BIGINT NOT NULL,
    latest_operation_id TEXT NOT NULL,
    latest_operation_index BIGINT NOT NULL,
    record_json TEXT NOT NULL
);

CREATE INDEX idx_federation_bark_vtxos_state
ON federation_bark_vtxos (state, latest_operation_index);

CREATE INDEX idx_federation_bark_vtxos_reservation
ON federation_bark_vtxos (reservation_action_id);

CREATE TABLE federation_bark_derivation_allocations (
    locator_role BIGINT NOT NULL,
    locator_index BIGINT NOT NULL,
    locator_version BIGINT NOT NULL,
    action_id TEXT NOT NULL,
    amount BIGINT NOT NULL,
    artifact_hash TEXT,
    vtxo_id TEXT UNIQUE,
    operation_id TEXT NOT NULL,
    operation_index BIGINT NOT NULL,
    record_json TEXT NOT NULL,
    PRIMARY KEY (locator_role, locator_index, locator_version)
);

CREATE INDEX idx_federation_bark_allocations_action
ON federation_bark_derivation_allocations (action_id, operation_index);

CREATE TABLE federation_bark_reservations (
    vtxo_id TEXT PRIMARY KEY NOT NULL,
    action_id TEXT NOT NULL,
    operation_id TEXT NOT NULL,
    operation_index BIGINT NOT NULL,
    amount BIGINT NOT NULL,
    record_json TEXT NOT NULL
);

CREATE INDEX idx_federation_bark_reservations_action
ON federation_bark_reservations (action_id, operation_index);

CREATE INDEX idx_federation_bark_reservations_operation
ON federation_bark_reservations (operation_id);

CREATE TABLE federation_bark_transitions (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_index BIGINT UNIQUE NOT NULL,
    action_id TEXT NOT NULL,
    phase TEXT NOT NULL,
    predecessor_operation_id TEXT UNIQUE,
    attempt BIGINT NOT NULL,
    record_json TEXT NOT NULL,
    UNIQUE (action_id, phase, attempt)
);

CREATE INDEX idx_federation_bark_transitions_action
ON federation_bark_transitions (action_id, operation_index);

CREATE TABLE federation_bark_observations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_index BIGINT UNIQUE NOT NULL,
    action_id TEXT NOT NULL,
    observer_member_id BIGINT NOT NULL,
    observation_epoch BIGINT NOT NULL,
    record_json TEXT NOT NULL,
    UNIQUE (action_id, observer_member_id, observation_epoch)
);

CREATE INDEX idx_federation_bark_observations_action
ON federation_bark_observations (action_id, operation_index);
