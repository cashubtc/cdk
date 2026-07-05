CREATE TABLE IF NOT EXISTS federation_member_status (
    member_id INTEGER PRIMARY KEY NOT NULL,
    status TEXT NOT NULL,
    peer_member_id INTEGER,
    local_operation_count INTEGER,
    peer_operation_count INTEGER,
    operation_count INTEGER,
    message TEXT,
    updated_time INTEGER NOT NULL
);
