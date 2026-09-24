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
list order and permits repeated points. Points are 33-byte secp256k1 or 48-byte BLS
G1 encodings; an empty list has no rows. Reads join this table with `transactions`,
and writes update both tables in one database transaction.

Migration `20260924000000` splits legacy concatenated secp256k1 or BLS blobs and
the JSON point arrays written by federation wallets into rows, then removes
`transactions.ys`. Mixed JSON arrays retain their point order and duplicates.
Older binaries cannot use the migrated SQL schema. Redb's JSON transaction format
is unchanged.

## License

This project is licensed under the [MIT License](../../LICENSE).
