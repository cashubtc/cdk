UPDATE proof SET witness = NULL
WHERE witness IS NOT NULL AND LENGTH(witness) > 1024;

UPDATE proof SET secret = ''
WHERE secret IS NOT NULL AND LENGTH(secret) > 1024;
