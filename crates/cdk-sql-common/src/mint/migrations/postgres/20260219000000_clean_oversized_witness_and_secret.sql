-- Clean up witness values exceeding 1024 characters
UPDATE proof SET witness = NULL WHERE LENGTH(witness) > 1024;

-- Clean up secret values exceeding 1024 characters
UPDATE proof SET secret = '' WHERE LENGTH(secret) > 1024;
