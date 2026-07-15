# Nutshell to CDK Mint Migration Plan

This document outlines the design, schema mapping, and implementation details for creating a `cdk-mintd` subcommand to automatically migrate a **Nutshell** mint database (SQLite or Postgres) to a **CDK** mint database.

---

## 1. Subcommand Design

The migration is exposed as a subcommand of `cdk-mintd` named `migrate-nutshell`.

```bash
# To migrate from a Nutshell SQLite database to the configured CDK database:
cdk-mintd --config path/to/cdk-config.toml migrate-nutshell --nutshell-db /path/to/nutshell/mint.sqlite3

# To migrate from a Nutshell Postgres database:
cdk-mintd --config path/to/cdk-config.toml migrate-nutshell --nutshell-db "postgres://user:pass@host:5432/nutshell_db"
```

### Flow of Execution:
1. **Initialize CLI and Configuration**: Parse command-line args and load the CDK target `Settings` (which configures the target database path/engine).
2. **Setup Target Database Schema**: Instantiate the CDK database using `setup_database()`. This automatically runs all target migrations so that all required CDK tables exist and are up to date.
3. **Connect to Source**: Establish a read-only connection to the source Nutshell database (SQLite or Postgres).
   The Nutshell mint must be stopped for the duration of the migration so the batched reads observe a stable database.
4. **Validation (Pre-flight Checks)**:
   - Check if the target database is already populated. If any target data exists (e.g. `proof`, `blind_signature`, `keyset`), abort the migration to prevent corruption.
   - Verify that the source database contains the standard Nutshell schema.
5. **Data Extraction & Migration**: Transfer the records from the source database to the target database in topological order inside a single database transaction.
6. **Auxiliary Index / Total Recovery**: Populate the CDK aggregate tables (like `keyset_amounts`) and history ledgers.
7. **Verify**: Run a sanity check comparing row counts and log completion.

---

## 2. Table and Field Mappings

Nutshell and CDK have slightly different table layouts and column types. Below is the mapping from Nutshell fields to CDK.

### A. Keysets (`keysets` $\to$ `keyset`)
| Nutshell Field | CDK Field | Type / Mapping Rule |
| :--- | :--- | :--- |
| `id` | `id` | `TEXT` |
| `unit` | `unit` | `TEXT` |
| `active` | `active` | `BOOL` (or `INTEGER` 1/0) |
| `valid_from` | `valid_from` | `INTEGER` (Unix timestamp, parsed from timestamp string/int) |
| `valid_to` | `valid_to` | `INTEGER` (Unix timestamp or NULL) |
| `derivation_path` | `derivation_path` | `TEXT` |
| `input_fee_ppk` | `input_fee_ppk` | `INTEGER` |
| `amounts` | `amounts` | `TEXT` (JSON array). If empty/`[]`, default to standard powers of two `[1, 2, ..., 2^31]` |
| *N/A* | `derivation_path_index` | `NULL` (or parsed if present) |
| *N/A* | `issuer_version` | `NULL` |

### B. Proofs (`proofs_used` & `proofs_pending` $\to$ `proof`)
Spent proofs are fetched from `proofs_used`, and pending proofs are fetched from `proofs_pending`.
| Nutshell Field | CDK Field | Type / Mapping Rule |
| :--- | :--- | :--- |
| `y` | `y` | `BLOB` (SQLite) / `BYTEA` (Postgres). **Must convert from hex string to raw bytes.** |
| `amount` | `amount` | `INTEGER` |
| `id` | `keyset_id` | `TEXT` |
| `secret` | `secret` | `TEXT` |
| `c` | `c` | `BLOB` (SQLite) / `BYTEA` (Postgres). **Must convert from hex string to raw bytes.** |
| `witness` | `witness` | `TEXT` |
| *N/A* | `state` | `'SPENT'` (for `proofs_used`) or `'PENDING'` (for `proofs_pending`) |
| `melt_quote` | `quote_id` | `TEXT` |
| `created` | `created_time` | `INTEGER` (Unix timestamp, parsed from timestamp) |
| *N/A* | `operation_kind` | `NULL` |
| *N/A* | `operation_id` | `NULL` |

### C. Promises (`promises` $\to$ `blind_signature`)
Nutshell promises are mapped directly to CDK's blind signatures.
| Nutshell Field | CDK Field | Type / Mapping Rule |
| :--- | :--- | :--- |
| `b_` | `blinded_message` | `BLOB` / `BYTEA`. **Must convert from hex string to raw bytes.** |
| `amount` | `amount` | `INTEGER` |
| `id` | `keyset_id` | `TEXT` |
| `c_` | `c` | `BLOB` / `BYTEA`. **Must convert from hex string to raw bytes.** |
| `mint_quote` | `quote_id` | `TEXT` |
| `dleq_e` | `dleq_e` | `TEXT` (Hex) |
| `dleq_s` | `dleq_s` | `TEXT` (Hex) |
| `created` | `created_time` | `INTEGER` (Unix timestamp) |
| `signed_at` | `signed_time` | `INTEGER` (Unix timestamp) |
| `order_index` | `order_index` | `INTEGER` |

### D. Mint Quotes (`mint_quotes` $\to$ `mint_quote` & auxiliary tables)
CDK represents mint quote state transitions with explicit amounts paid/issued, and tracks events in separate tables.
| Nutshell Field | CDK Field | Type / Mapping Rule |
| :--- | :--- | :--- |
| `quote` | `id` | `TEXT` |
| `amount` | `amount` | `INTEGER` |
| `unit` | `unit` | `TEXT` |
| `request` | `request` | `TEXT` |
| `checking_id` | `request_lookup_id` | `TEXT` |
| `state` | `state` | `TEXT` (Uppercase: `"UNPAID"`, `"PAID"`, `"ISSUED"`) |
| `pubkey` | `pubkey` | `TEXT` |
| `created_time` | `created_time` | `INTEGER` (Unix timestamp) |
| *N/A* | `expiry` | Default to `created_time + 86400` |
| *N/A* | `request_lookup_id_kind` | `"payment_hash"` if 32-byte hex, otherwise `"custom"` |
| *N/A* | `amount_paid` | `0` if unpaid; `amount` if paid/issued |
| *N/A* | `amount_issued` | `0` if unpaid/paid; `amount` if issued |

#### Auxiliary Mint Quote Tables:
- **`mint_quote_payments`**: If quote is `"PAID"` or `"ISSUED"`, write a row representing the event:
  - `quote_id` = `quote`
  - `payment_id` = `checking_id`
  - `amount` = `amount`
  - `timestamp` = `paid_time` (or `created_time` if null)
- **`mint_quote_issued`**: If quote is `"ISSUED"`, write a row representing the event:
  - `quote_id` = `quote`
  - `amount` = `amount`
  - `timestamp` = `issued_time` (or `paid_time` / `created_time` if null)

### E. Melt Quotes (`melt_quotes` $\to$ `melt_quote`)
| Nutshell Field | CDK Field | Type / Mapping Rule |
| :--- | :--- | :--- |
| `quote` | `id` | `TEXT` |
| `unit` | `unit` | `TEXT` |
| `amount` | `amount` | `INTEGER` |
| `request` | `request` | `TEXT` (Nutshell stores raw bolt11 string; CDK parses this natively) |
| `fee_reserve` | `fee_reserve` | `INTEGER` |
| `state` | `state` | `TEXT` (Uppercase: `"UNPAID"`, `"PENDING"`, `"PAID"`, `"FAILED"`) |
| `expiry` | `expiry` | `INTEGER` (Unix timestamp) |
| `created_time` | `created_time` | `INTEGER` (Unix timestamp) |
| `paid_time` | `paid_time` | `INTEGER` (Unix timestamp or NULL) |
| `payment_proof` | `payment_proof` | `TEXT` (Preimage, or NULL) |

---

## 3. Handling Critical Edge Cases

1. **Chunked Pagination / Bounded Memory Streaming**: To prevent out-of-memory errors on massive nutshell databases (like Minibits), we retrieve all large datasets (`mint_quotes`, `melt_quotes`, `promises`, `proofs_used`, `proofs_pending`) in constant-sized batches of 2000 rows. This guarantees flat, highly predictable, and negligible memory overhead throughout the entire process.
2. **Pre-flight Corruption Check**: We query `SELECT COUNT(*) FROM proof` and `keyset` on the target DB. If any target data exists (e.g. `proof`, `blind_signature`, `keyset`), abort the migration to prevent corruption.
3. **Byte Conversions**: Binary columns are decoded from Hex strings to raw byte vectors (`Vec<u8>`) to ensure database queries in CDK continue to hit indexes correctly.
4. **Aggregate Keyset Metrics Recovery**: To repopulate `keyset_amounts` in CDK, we run three aggregate insert/update statements at the end of the migration:
   - Compute total issued per keyset from `blind_signature`
   - Compute total redeemed per keyset from spent `proof`s
5. **Resilient Date / Timestamp Parsing**:
   - Check if a timestamp value is integer-string; if so, parse as unix timestamp.
   - If stored as string (e.g. `"2026-05-12 14:00:23.123"`), parse with format specifiers (e.g., `"%Y-%m-%d %H:%M:%S"` or `"%Y-%m-%d %H:%M:%S.%f"`) to convert to standard epoch seconds.

---

## 4. Implementation Steps

1. **Direct DB Connectors**: Direct, performant connections to target and source databases are handled with `rusqlite` and `tokio-postgres`.
2. **Cargo configuration**: Include database driver dependencies under target compile features (`sqlite`/`postgres`) in `cdk-mintd/Cargo.toml`.
3. **Register Subcommand**: Add `Subcommand::MigrateNutshell` to `cli.rs` and configure `main.rs` to route to `migrate::run_migration(...)` if specified.
4. **Implement Migration Logic**: Create `cdk-mintd/src/migrate.rs` containing chunk-paged query mappings, row-decoders, and transactional bulk insertion logic.
