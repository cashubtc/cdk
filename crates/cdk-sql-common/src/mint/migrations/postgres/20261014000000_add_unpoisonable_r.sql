ALTER TABLE proof ADD COLUMN unpoisonable_r BYTEA
    CHECK (unpoisonable_r IS NULL OR (octet_length(unpoisonable_r) = 32 AND state = 'SPENT'));
