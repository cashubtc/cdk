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


## License

This project is licensed under the [MIT License](../../LICENSE).
