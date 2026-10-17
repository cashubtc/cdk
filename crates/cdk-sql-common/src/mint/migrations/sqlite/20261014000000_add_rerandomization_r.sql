ALTER TABLE proof ADD COLUMN rerandomization_r BLOB
    CHECK (rerandomization_r IS NULL OR (length(rerandomization_r) = 32 AND state = 'SPENT'));
