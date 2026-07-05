CREATE TABLE IF NOT EXISTS federation_member_status (
    member_id BIGINT PRIMARY KEY NOT NULL,
    status TEXT NOT NULL,
    peer_member_id BIGINT,
    local_operation_count BIGINT,
    peer_operation_count BIGINT,
    operation_count BIGINT,
    message TEXT,
    updated_time BIGINT NOT NULL
);
