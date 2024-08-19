
# cdk-mintd

## Building from source

```sh
  nix develop
```

#### Build

```sh
  cargo b --bin cdk-mintd -r
```

## Configuration

### Copy example config to cdk-mintd working directory.

```sh
  cp ./example.config.toml ~/.cdk-mintd/config.toml
```

### Edit config file

```sh
  vi ~/.cdk-mintd/config.toml
```

## Greenlight

Create a greenlight working directory

```sh
  mkdir ~/.cdk-mintd/greenlight
```

Include the `client.crt` and `client-key.pem` in the greenlight working directory.
These can be downloaded from <https://blockstream.github.io/greenlight/getting-started/certs/>
