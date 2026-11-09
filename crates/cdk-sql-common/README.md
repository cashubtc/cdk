# CDK SQL Base

This is a private crate offering a common framework to interact with SQL databases.

This crate uses standard SQL, a generic migration framework a traits to implement blocking or
non-blocking clients.


**ALPHA** This library is in early development, the API will change and should be used with caution.

## Features

The following crate feature flags are available:

| Feature     | Default | Description                        |
|-------------|:-------:|------------------------------------|
| `wallet`    |   Yes   | Enable cashu wallet features       |
| `mint`      |   Yes   | Enable cashu mint wallet features  |
| `auth`      |   Yes   | Enable cashu mint auth features    |


## Wallet transaction proof identifiers

SQLite and Postgres store transaction proof identifiers in `transaction_ys`, with
one raw curve point per row. The `(transaction_id, position)` primary key preserves
list order and permits repeated points. Each row currently holds a 33-byte
secp256k1 point; an empty list has no rows. Storing points separately removes the
fixed-width concatenated layout, while retaining validation of supported point
encodings. Reads join this table with `transactions`, and writes update both
tables in one database transaction.

Migration `20260924000000` splits existing concatenated secp256k1 blobs into rows
and removes `transactions.ys`. Malformed trailing fragments cause the migration
to fail and roll back. Older binaries cannot use the migrated SQL schema; retain
a pre-upgrade database backup if downgrade is required. Redb's JSON transaction
format is unchanged.

Updating an existing transaction replaces its proof identifiers as well as its
metadata. Transaction listing reports decoding errors instead of silently
omitting malformed records.

## License

This project is licensed under the [MIT License](../../LICENSE).
