ALTER TABLE proof ADD COLUMN unpoisonable_r BLOB
    CHECK (unpoisonable_r IS NULL OR (length(unpoisonable_r) = 32 AND state = 'SPENT'));
