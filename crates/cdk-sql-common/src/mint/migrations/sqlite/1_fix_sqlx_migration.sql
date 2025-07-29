-- Migrate `_sqlx_migrations` to our new migration system
CREATE TABLE IF NOT EXISTS _sqlx_migrations AS
SELECT
    '' AS version,
    '' AS description,
    0 AS execution_time
WHERE 0;

INSERT INTO migrations
SELECT
    version || '_' ||  REPLACE(description, ' ', '_') || '.sql',
    execution_time
FROM _sqlx_migrations
WHERE EXISTS (
    SELECT 1
    FROM sqlite_master
    WHERE type = 'table' AND name = '_sqlx_migrations'
);

DROP TABLE _sqlx_migrations;
