-- Delete duplicates from `mint`
DELETE FROM `mint`
WHERE `mint_url` IN (
    SELECT `mint_url`
    FROM (
        SELECT RTRIM(`mint_url`, '/') AS trimmed_url, MIN(rowid) AS keep_id
        FROM `mint`
        GROUP BY trimmed_url
        HAVING COUNT(*) > 1
    )
)
AND rowid NOT IN (
    SELECT MIN(rowid)
    FROM `mint`
    GROUP BY RTRIM(`mint_url`, '/')
);

UPDATE `mint` SET `mint_url` = RTRIM(`mint_url`, '/');
UPDATE `keyset` SET `mint_url` = RTRIM(`mint_url`, '/');
UPDATE `mint_quote` SET `mint_url` = RTRIM(`mint_url`, '/');
UPDATE `proof` SET `mint_url` = RTRIM(`mint_url`, '/');
