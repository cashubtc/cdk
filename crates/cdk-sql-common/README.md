# CDK SQL Base

This is a private crate offering a common framework to interact with SQL databases.

This crate uses standard SQL, a generic migration framework, and traits to implement blocking or
non-blocking clients.

## Reversible migrations

Forward migrations keep the existing `<timestamp>_<name>.sql` naming convention. A reversible
migration has a second file beside it named `<timestamp>_<name>.down.sql`. The build script embeds
the pair as one migration; `.down.sql` files are never applied during normal database startup.

Use `just new-migration mint <name>` or `just new-migration wallet <name>` to create both files.
If a migration cannot be reversed without unsafe or ambiguous data reconstruction, remove the
empty `.down.sql` file. A rollback that would cross such a migration is rejected before any schema
changes are made.

Backward SQL must undo only its paired forward migration and must work inside a database
transaction on every backend directory in which it is provided. Test both the rollback and a
subsequent forward reapplication.

**ALPHA** This library is in early development, the API will change and should be used with caution.

## Features

The following crate feature flags are available:

| Feature     | Default | Description                        |
|-------------|:-------:|------------------------------------|
| `wallet`    |   Yes   | Enable cashu wallet features       |
| `mint`      |   Yes   | Enable cashu mint wallet features  |
| `auth`      |   Yes   | Enable cashu mint auth features    |


## License

This project is licensed under the [MIT License](../../LICENSE).
