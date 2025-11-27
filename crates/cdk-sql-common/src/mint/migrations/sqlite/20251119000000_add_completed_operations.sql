-- Create completed_operations table to track finished operations
CREATE TABLE IF NOT EXISTS completed_operations (
    operation_id TEXT PRIMARY KEY NOT NULL,
    operation_kind TEXT NOT NULL,
    completed_at INTEGER NOT NULL,
    total_issued INTEGER NOT NULL,
    total_redeemed INTEGER NOT NULL,
    fee_collected INTEGER NOT NULL,
    payment_amount INTEGER,
    payment_fee INTEGER,
    payment_method TEXT
);

-- Create index for efficient querying by operation kind and time
CREATE INDEX IF NOT EXISTS idx_completed_operations_kind_time ON completed_operations(operation_kind, completed_at);

-- Create index for time-based queries
CREATE INDEX IF NOT EXISTS idx_completed_operations_time ON completed_operations(completed_at);
