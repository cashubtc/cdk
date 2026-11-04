ALTER TABLE proof ADD COLUMN rerandomization_r BYTEA
    CHECK (rerandomization_r IS NULL OR (octet_length(rerandomization_r) = 32 AND state = 'SPENT'));
