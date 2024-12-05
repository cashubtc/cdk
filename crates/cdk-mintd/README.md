
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

## Running with Docker 

### Build the Docker image

```sh
  docker build -t cdk-mintd .
```

### Run the Docker container with the configuration file mapped

```sh
  docker run -v ~/.cdk-mintd/config.toml:/root/.cdk-mintd/config.toml cdk-mintd
```
