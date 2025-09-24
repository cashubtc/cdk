-- Add unique constraint to blinded_message column to prevent duplicates at DB level
-- This ensures no duplicate blinded messages can be inserted, complementing application-level checks

CREATE UNIQUE INDEX IF NOT EXISTS idx_blinded_message_unique
ON blinded_messages(blinded_message);
