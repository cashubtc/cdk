-- Migration to add DLEQ proof storage to the proof table
ALTER TABLE proof ADD COLUMN dleq_e TEXT;
ALTER TABLE proof ADD COLUMN dleq_s TEXT;
ALTER TABLE proof ADD COLUMN dleq_r TEXT;
