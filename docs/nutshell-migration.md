# Migrating a Nutshell mint to CDK

`cdk-mintd` can migrate Nutshell 0.20.2 keysets, quotes, promises, and pending or
spent proofs into an empty CDK database of the same type.

Stop the Nutshell mint and back up its database before starting. Keep it stopped
until cutover because the migration reads the source in batches and requires a
stable snapshot.

## SQLite

```bash
cdk-mintd --work-dir /path/to/cdk migrate-nutshell \
  --nutshell-db /path/to/nutshell/mint.db
```

The target is `/path/to/cdk/cdk-mintd.sqlite`. Without `--work-dir`, CDK uses
its default work directory (`~/.cdk-mintd`).

## PostgreSQL

Configure an empty CDK PostgreSQL target in `config.toml`:

```toml
[database]
engine = "postgres"

[database.postgres]
url = "postgresql://cdk_user:password@localhost:5432/cdk_mint"
```

Then pass the Nutshell source connection string:

```bash
cdk-mintd migrate-nutshell \
  --nutshell-db "postgresql://nutshell_user:password@localhost:5432/nutshell_mint"
```

Both source and target URLs honor their `sslmode` setting. Cross-database
migrations (SQLite to PostgreSQL or the reverse) are not supported.

## Verification

Migration performs an independent verification pass. Run it again without
writing data using:

```bash
cdk-mintd --work-dir /path/to/cdk migrate-nutshell \
  --nutshell-db /path/to/nutshell/mint.db \
  --verify-only
```

## Configure the mint seed

CDK must use the original Nutshell `MINT_PRIVATE_KEY` so existing notes remain
valid. Set it before starting the migrated mint:

```bash
CDK_MINTD_SEED="your_nutshell_mint_private_key" \
  cdk-mintd --work-dir /path/to/cdk
```

Alternatively, set `seed` in the `[info]` section of `config.toml`.
