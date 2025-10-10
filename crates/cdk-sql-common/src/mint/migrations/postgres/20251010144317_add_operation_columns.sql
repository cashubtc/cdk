-- Add operation and operation_id columns to proof table
ALTER TABLE proof ADD COLUMN operation_kind TEXT;
ALTER TABLE proof ADD COLUMN operation_id TEXT;

-- Add operation and operation_id columns to blind_signature table
ALTER TABLE blind_signature ADD COLUMN operation_kind TEXT;
ALTER TABLE blind_signature ADD COLUMN operation_id TEXT;
