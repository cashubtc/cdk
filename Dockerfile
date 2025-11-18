# Use the official NixOS image as the base image
FROM nixos/nix:latest AS builder

# Set the working directory
WORKDIR /usr/src/app

# Copy workspace files and crates directory into the container
COPY flake.nix ./flake.nix
COPY Cargo.toml ./Cargo.toml
COPY crates ./crates

# Start the Nix daemon and develop the environment
RUN nix develop --extra-experimental-features nix-command --extra-experimental-features flakes --command cargo build --release --bin cdk-mintd --features postgres --features prometheus

# Create a runtime stage
FROM debian:trixie-slim

# Set the working directory
WORKDIR /usr/src/app

# Install needed runtime dependencies (if any)
RUN apt-get update && \
    apt-get install -y --no-install-recommends patchelf && \
    rm -rf /var/lib/apt/lists/*

# Copy the built application from the build stage
COPY --from=builder /usr/src/app/target/release/cdk-mintd /usr/local/bin/cdk-mintd

# Detect the architecture and set the interpreter accordingly
RUN ARCH=$(uname -m) && \
    if [ "$ARCH" = "aarch64" ]; then \
        patchelf --set-interpreter /lib/ld-linux-aarch64.so.1 /usr/local/bin/cdk-mintd; \
    elif [ "$ARCH" = "x86_64" ]; then \
        patchelf --set-interpreter /lib64/ld-linux-x86-64.so.2 /usr/local/bin/cdk-mintd; \
    else \
        echo "Unsupported architecture: $ARCH"; exit 1; \
    fi

# Set the entry point for the container
CMD ["cdk-mintd"]
