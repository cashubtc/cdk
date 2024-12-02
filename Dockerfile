# Use the official Rust image as a base
FROM rust:1.82-slim-bullseye AS builder

# Create a new empty shell project
WORKDIR /usr/src/app

# Copy the workspace files
COPY Cargo.toml ./

# Copy the crate directory
COPY crates ./crates

RUN apt-get update \
    && apt-get install -y protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*
# Build dependencies
RUN cargo build --release -p cdk-mintd && \
    rm -f target/release/deps/cdk_mintd*

# Build the actual application
RUN cargo build --release -p cdk-mintd

# Create a new stage with a minimal image 
FROM debian:bullseye-slim

# Install needed runtime dependencies (if any)
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /usr/src/app/target/release/cdk-mintd /usr/local/bin/cdk-mintd

# Set the startup command
CMD ["cdk-mintd"]
