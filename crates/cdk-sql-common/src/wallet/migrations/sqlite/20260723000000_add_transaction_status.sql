ALTER TABLE transactions
ADD COLUMN status TEXT NOT NULL DEFAULT 'completed'
CHECK (status IN ('pending', 'completed', 'failed'));
