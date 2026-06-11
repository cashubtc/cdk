-- Post-migration role password setup for the local Supabase test stack.
--
-- The supabase/postgres image creates the internal service roles
-- (authenticator, supabase_auth_admin, ...) without login passwords; only the
-- supabase_admin superuser gets POSTGRES_PASSWORD. GoTrue and PostgREST connect
-- as those service roles, so they fail with "password authentication failed"
-- until the passwords are set.
--
-- migrate.sh runs this file (mounted at /etc/postgresql.schema.sql) after all
-- migrations, which is the image's documented hook for exactly this purpose.
--
-- The password MUST match the POSTGRES_PASSWORD default ("supabase") used by the
-- auth/rest connection strings in docker-compose.yml.

ALTER USER authenticator WITH PASSWORD 'supabase';
ALTER USER supabase_auth_admin WITH PASSWORD 'supabase';
ALTER USER supabase_storage_admin WITH PASSWORD 'supabase';
ALTER USER supabase_replication_admin WITH PASSWORD 'supabase';
ALTER USER supabase_read_only_user WITH PASSWORD 'supabase';
ALTER USER pgbouncer WITH PASSWORD 'supabase';
