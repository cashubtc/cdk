-- Add unique constraint to blinded_message column to prevent duplicates at DB level
-- This ensures no duplicate blinded messages can be inserted, complementing application-level checks

ALTER TABLE blinded_messages 
ADD CONSTRAINT unique_blinded_message 
UNIQUE (blinded_message);
