# Use the official NixOS image as the base image
FROM nixos/nix:latest AS builder

# Set the working directory
WORKDIR /usr/src/app

# Copy the source code and flake.nix into the container
COPY . /usr/src/app

# Start the Nix daemon and develop the environment
RUN nix-channel --update && \
    nix-env -iA nixpkgs.nix && \
    nix develop --extra-experimental-features nix-command --extra-experimental-features flakes --command cargo build --release --bin cdk-mintd

# Create a runtime stage
FROM debian:bullseye-slim

# Set the working directory
WORKDIR /usr/src/app

# Copy the built application from the build stage
COPY --from=builder /usr/src/app/target/release/cdk-mintd /usr/src/app/target/release/cdk-mintd

# Set the entry point for the container
CMD ["/usr/src/app/target/release/cdk-mintd"]