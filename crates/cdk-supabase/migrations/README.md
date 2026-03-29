# Supabase Migrations

These SQL files define the CDK wallet database schema for Supabase.

Migration files live in `supabase/migrations/` using the Supabase CLI timestamp
format (`YYYYMMDDHHmmss_description.sql`). The Rust SDK embeds these same files
via `build.rs` for use with `get_schema_sql()`.

## Running Migrations (Operator / Admin Only)

Migrations must be run by an operator with admin access to the Supabase project.
**Never run migrations from a client application.**

### Option 1 — Supabase CLI (recommended)

Run from the `migrations/` directory (where `supabase/.temp/project-ref` lives):

```bash
supabase db push
```

> **Important:** The CLI must connect via the **direct database URL**, not the
> Supavisor connection pooler. Supavisor runs in transaction mode and rejects
> prepared statements that contain multiple SQL commands (SQLSTATE 42601).
> If you see this error, pass the direct URL explicitly:
>
> ```bash
> supabase db push --db-url "postgresql://postgres:[PASSWORD]@db.[PROJECT-REF].supabase.co:5432/postgres"
> ```
>
> Find the direct connection string in the Supabase Dashboard under
> **Project Settings → Database → Connection string → Direct connection**.

### Option 2 — Supabase Dashboard SQL editor

Use the Rust SDK helper to get the full concatenated SQL:

```rust
let sql = SupabaseWalletDatabase::get_schema_sql();
// or via FFI:
let sql = supabase_get_schema_sql();
```

Paste the output into the **SQL Editor** in the Supabase Dashboard and run it.

### Option 3 — Manual file-by-file

Run each `*.sql` file in the `supabase/migrations/` directory in timestamp order
using the Supabase Dashboard SQL editor or `psql` with the service role
connection string.

## Client-Side Compatibility Check

After an operator has run migrations, client applications should call
`check_schema_compatibility()` before first use:

```rust
db.check_schema_compatibility().await?;
```

This reads the `schema_info` table (readable by all authenticated users) and
returns an error if the database schema is older than what the SDK requires.

## Adding New Migrations

1. Create a new timestamped file in `supabase/migrations/`:
   ```
   supabase/migrations/YYYYMMDDHHmmss_description.sql
   ```
2. If the migration contains a function definition (`CREATE OR REPLACE FUNCTION
   ... AS $body$ ... $body$;`), **put it in its own file with no other
   statements after the closing `$body$;`**. Supavisor rejects prepared
   statements with multiple commands, and the CLI batches everything after a
   function definition into one statement. One function per file avoids this.
3. If the migration changes the schema in a user-visible way, update the
   `schema_info` version at the end of the file:
   ```sql
   INSERT INTO schema_info (key, value) VALUES ('schema_version', 'N')
   ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;
   ```
4. Bump `REQUIRED_SCHEMA_VERSION` in `crates/cdk-supabase/src/wallet.rs`.

## Supavisor Prepared-Statement Limitation

The Supabase CLI connects through **Supavisor** (the connection pooler) by
default. Supavisor enforces that each prepared statement contains exactly one
SQL command. This causes `SQLSTATE 42601` when the CLI sends a batch containing
a function definition followed by other statements (e.g. `GRANT`, `ALTER TABLE`,
`CREATE POLICY`).

The migration files in this repo work around this by placing each function in
its own file. If you encounter this error when adding new migrations, the fix
is to split the offending file so each function definition is the only statement
in its file.

Using a direct database connection (bypassing Supavisor) also resolves the
issue — see the `--db-url` option above.
