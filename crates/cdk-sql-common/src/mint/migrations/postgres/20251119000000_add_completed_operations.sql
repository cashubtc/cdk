-- Create completed_operations table to track finished operations
CREATE TABLE IF NOT EXISTS completed_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_kind TEXT NOT NULL,
    completed_at BIGINT NOT NULL,
    total_issued BIGINT NOT NULL,
    total_redeemed BIGINT NOT NULL,
    fee_collected BIGINT NOT NULL,
    payment_amount BIGINT,
    payment_fee BIGINT,
    payment_method TEXT
);

-- Create index for efficient querying by operation kind and time
CREATE INDEX IF NOT EXISTS idx_completed_operations_kind_time ON completed_operations(operation_kind, completed_at);

-- Create index for time-based queries
CREATE INDEX IF NOT EXISTS idx_completed_operations_time ON completed_operations(completed_at);
