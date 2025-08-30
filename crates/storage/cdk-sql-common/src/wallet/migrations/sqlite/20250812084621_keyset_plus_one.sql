-- Increment keyset counter by 1 where counter > 0
UPDATE keyset SET counter = counter + 1 WHERE counter > 0;
