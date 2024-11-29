# Use the official Ubuntu 24.04 image as the base image
FROM ubuntu:24.04

# Set the working directory
WORKDIR /usr/src/app

# Install necessary dependencies
RUN apt-get update && apt-get install -y \
    curl \
    protobuf-compiler \
    xz-utils

# Copy the source code and flake.nix into the container
COPY . /usr/src/app

# Create /nix directory and set permissions
USER root
RUN mkdir -m 0755 /nix && chown root /nix

# Create nixbld group and users
RUN groupadd -g 30000 nixbld && \
    for i in $(seq 1 10); do \
        useradd -u $((30000 + i)) -g nixbld -G nixbld -m -s /bin/false nixbld$i; \
    done

# Install Nix and set up the environment
RUN curl -L https://nixos.org/nix/install | sh -s -- --daemon && \
    /bin/bash -c "source /root/.nix-profile/etc/profile.d/nix.sh && \
    exec bash -l -c 'nix-channel --update && nix-env -iA nixpkgs.nix'"

# Start the Nix daemon and develop the environment
RUN /bin/bash -c "source /root/.nix-profile/etc/profile.d/nix.sh && \
    /root/.nix-profile/bin/nix-daemon & \
    cd /usr/src/app && \
    /root/.nix-profile/bin/nix develop --extra-experimental-features nix-command --extra-experimental-features flakes --command cargo build --release --bin cdk-mintd"

# Set the entry point for the container
CMD ["/usr/src/app/target/release/cdk-mintd"]