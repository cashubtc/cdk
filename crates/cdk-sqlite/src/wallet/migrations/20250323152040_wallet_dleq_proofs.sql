-- Migration to add DLEQ proof storage to the proof table
ALTER TABLE proof ADD COLUMN dleq_e BLOB;
ALTER TABLE proof ADD COLUMN dleq_s BLOB;
ALTER TABLE proof ADD COLUMN dleq_r BLOB;
