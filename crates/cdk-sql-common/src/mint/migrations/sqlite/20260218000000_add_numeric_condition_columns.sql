-- NUT-CTF-numeric: Add numeric condition columns to conditions table
ALTER TABLE conditions ADD COLUMN condition_type TEXT NOT NULL DEFAULT 'enum';
ALTER TABLE conditions ADD COLUMN lo_bound INTEGER;
ALTER TABLE conditions ADD COLUMN hi_bound INTEGER;
ALTER TABLE conditions ADD COLUMN precision INTEGER;
