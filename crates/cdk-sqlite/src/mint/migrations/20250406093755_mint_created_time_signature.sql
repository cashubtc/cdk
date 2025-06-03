-- Add created_time column to blind_signature table
ALTER TABLE blind_signature ADD COLUMN created_time INTEGER NOT NULL DEFAULT 0;
-- Add created_time column to proof table
ALTER TABLE proof ADD COLUMN created_time INTEGER NOT NULL DEFAULT 0;
