# Development Guide

This guide will help you set up your development environment for working with the CDK repository.

## Prerequisites

Before you begin, ensure you have:
- Git installed on your system
- GitHub account
- Basic familiarity with command line operations

## Initial Setup

### 1. Fork and Clone the Repository

1. Navigate to the CDK repository on GitHub
2. Click the "Fork" button in the top-right corner
3. Clone your forked repository:
```bash
git clone https://github.com/YOUR-USERNAME/cdk.git
cd cdk
```

### 2. Install Nix

<!-- 
MIT License

Copyright (c) 2021 elsirion
https://github.com/fedimint/fedimint/blob/master/docs/dev-env.md
-->

CDK uses [Nix](https://nixos.org/explore.html) for building, CI, and managing dev environment.
Note: only `Nix` (the language & package manager) and not the NixOS (the Linux distribution) is needed.
Nix can be installed on any Linux distribution and macOS.

While Nix is preferred as it ensures a consistent and reproducible environment
for all developers, it is not strictly required to use Nix to build CDK.

### Install Nix

You have 2 options to install nix:

* **RECOMMENDED:** The [Determinate Nix Installer](https://github.com/DeterminateSystems/nix-installer)
* [The official installer](https://nixos.org/download.html)

Example:

```
> nix --version
nix (Nix) 2.9.1
```

The exact version might be different.

### Enable nix flakes

If you installed Nix using the "determinate installer" you can skip this step. If you used the "official installer", edit either `~/.config/nix/nix.conf` or `/etc/nix/nix.conf` and add:

```
experimental-features = nix-command flakes
```

If the Nix installation is in multi-user mode, don’t forget to restart the nix-daemon.

## Alternative Setup Without Nix

While Nix is preferred as it ensures a consistent and reproducible environment
for all developers, it is not strictly required to use Nix to build CDK. You can
also set up your environment manually.

### Installing Rust via rustup

To build CDK without Nix, you'll need to install Rust manually:

1. Install rustup by following the instructions at [https://www.rust-lang.org/tools/install](https://www.rust-lang.org/tools/install)

2. Once rustup is installed, you can install the required Rust version:
```bash
rustup install stable
rustup default stable
```

3. Install required tools:
```bash
# For building cdk-mintd, you'll need protobuf compiler
# On Ubuntu/Debian:
sudo apt install protobuf-compiler

# On macOS with Homebrew:
brew install protobuf

# On other systems, please refer to your package manager or
# https://grpc.io/docs/protoc-installation/
```

### Building and Running CDK Components

#### Building cdk-cli

To build the CDK command-line interface:
```bash
cargo build --bin cdk-cli --release
```

To run cdk-cli directly without building:
```bash
cargo run --bin cdk-cli -- --help
```

#### Building cdk-mintd

To build the CDK mint server:
```bash
cargo build --bin cdk-mintd --release
```

To run cdk-mintd directly without building:
```bash
cargo run --bin cdk-mintd
```

Note: For cdk-mintd, you need to have the protobuf compiler installed as it's required for some dependencies.

## Nix Development Environments

CDK uses Nix flakes to provide reproducible development environments. We offer a tiered shell strategy to ensure developers only download what they need for their specific tasks.

### Available Shells

| Shell | Command | Key Tools Included | Best For |
| :--- | :--- | :--- | :--- |
| **Stable (Default)** | `nix develop` | Rust Stable, **PostgreSQL**, Protobuf | Library, Wallet, and Mint development |
| **Regtest** | `nix develop .#regtest` | Stable + **bitcoind, CLN, LND, mprocs** | Local integration testing and nodes |
| **Nightly** | `nix develop .#nightly` | Rust Nightly, **PostgreSQL** | Formatting and experimental features |
| **Nightly Regtest** | `nix develop .#nightly-regtest` | Nightly + **Full Regtest Stack** | Comprehensive testing on nightly |
| **Integration** | `nix develop .#integration` | Stable + Regtest Stack + **Docker** | Binding, Auth, and Docker tests |
| **MSRV** | `nix develop .#msrv` | Rust 1.85.0 (Minimum version) | Ensuring backward compatibility |
| **FFI** | `nix develop .#ffi` | Stable + Python 3.11 | Working on UniFFI bindings |

### PostgreSQL Helpers

All CDK shells come with a built-in PostgreSQL instance and helper scripts to manage it locally within your project directory:

- `start-postgres`: Initialize and start a local PostgreSQL instance (data stored in `.pg_data/`).
- `stop-postgres`: Safely shut down the local database.
- `pg-status`: Check if the database is running.
- `pg-connect`: Open an interactive `psql` session to the local database.

### Direct Flake Commands

You can build project components directly using flake targets without manually entering a shell:

```bash
# Build the wallet CLI
nix build .#cdk-cli
./result/bin/cdk-cli --help

# Build the mint daemon
nix build .#cdk-mintd

# Run a check (clippy + tests) for a specific crate
nix build .#checks.x86_64-linux.cashu
```

### Nix Troubleshooting

- **Updating Dependencies**: If you notice dependencies are out of date or a new tool has been added to the flake, run `nix flake update` to refresh the `flake.lock` file.
- **Command Not Found**: Ensure you have entered the shell (e.g., `nix develop`). Some tools like `mprocs` or `bitcoind` are only available in the `regtest` shell.
- **Cache Issues**: If you suspect the environment is not reflecting recent flake changes, you can force a re-evaluation with `nix flake check`.
- **Persistent Data**: The local PostgreSQL instance stores data in the `.pg_data/` directory. If you want to reset your database completely, stop the database and delete this directory.

## Regtest Environment

For testing and development, CDK provides a complete regtest environment with Bitcoin, Lightning Network nodes, and CDK mints.

### Quick Start
```bash
just regtest  # Starts full environment with mprocs TUI
```

This provides:
- Bitcoin regtest node
- 4 Lightning Network nodes (2 CLN + 2 LND)
- 2 CDK mints (one connected to CLN, one to LND)
- Real-time log monitoring via mprocs
- Helper commands for testing Lightning payments and CDK operations

### Comprehensive Guide
See [REGTEST_GUIDE.md](REGTEST_GUIDE.md) for complete documentation including:
- Detailed setup and usage instructions
- Development workflows and testing scenarios
- mprocs TUI interface guide
- Troubleshooting and advanced usage

## Common Development Tasks

### Building the Project
```sh
just build
```

### Running Unit Tests
```bash
just test
```

### Running Integration Tests
```bash
just itest REDB/SQLITE/MEMORY
```

NOTE: if this command fails on macos change the nix channel to unstable (in the `flake.nix` file modify `nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";` to `nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";`)

### Running Mutation Tests

Mutation testing validates test suite quality by introducing small code changes (mutations) and verifying tests catch them.

```bash
# Run mutation tests on cashu crate (configured in .cargo/mutants.toml)
cargo mutants

# Check specific files only
cargo mutants --file crates/cashu/src/amount.rs

# Re-run previously caught mutations to verify fixes
cargo mutants --in-diff
```

**Understanding Results:**
- **Caught mutations**: Tests correctly detected the code change (good!)
- **Missed mutations**: Code change went undetected - indicates missing test coverage
- **Timeouts**: Mutation caused infinite loop - some are excluded in config to keep tests practical

The `.cargo/mutants.toml` file excludes mutations that cause infinite loops during testing. These don't indicate bugs - they're just mutations that would make the test suite hang indefinitely.

See [cargo-mutants documentation](https://mutants.rs/) for more options.

### Running Format
```bash
just format
```

## Code Formatting

CDK uses a flexible rustfmt policy to balance code quality with developer experience:

### Formatting Requirements for PRs
Pull requests can be formatted with **either stable or nightly** rustfmt - both are accepted:

- **Stable rustfmt:** Standard Rust formatting (less strict)
- **Nightly rustfmt:** More strict formatting with additional rules

**Why both are accepted:**
- We prefer nightly rustfmt's stricter formatting
- We don't want to force contributors to install nightly Rust
- This reduces friction for developers using stable toolchains

```bash
# Format with stable (default)
just format

# Format with nightly (if you have it installed)
cargo +nightly fmt
```

The CI will check your PR with stable rustfmt, so as long as your code passes stable formatting, your PR will pass CI.

### Automated Nightly Formatting
To keep the codebase consistently formatted with nightly rustfmt over time:

- **Daily Check:** Every night at midnight UTC, a GitHub Action runs nightly rustfmt on the `main` branch
- **Automated PRs:** If nightly rustfmt produces formatting changes, a PR is automatically created with:
  - Title: `Automated nightly rustfmt (YYYY-MM-DD)`
  - Label: `rustfmt`
  - Author: `Fmt Bot <bot@cashudevkit.org>`
- **Review Process:** These automated PRs are reviewed and merged to keep the codebase aligned with nightly formatting

This approach ensures the codebase gradually adopts nightly formatting improvements without blocking contributors who use stable Rust.


### Running Clippy
```bash
just clippy
```

### Running final check before commit
```sh
just final-check
```


## Best Practices

1. **Branch Management**
   - Create feature branches from `main`
   - Use descriptive branch names: `feature/new-feature` or `fix/bug-description`

2. **Commit Messages**
   - Follow conventional commits format
   - Begin with type: `feat:`, `fix:`, `docs:`, `chore:`, etc.
   - Provide clear, concise descriptions

3. **Testing**
   - Write tests for new features
   - Ensure all tests pass before submitting PR
   - Include integration tests where applicable

## Troubleshooting

### Common Issues

1. **Development Shell Issues**
   - Clean Nix store: `nix-collect-garbage -d`
   - Remove and recreate development shell

### Getting Help

- Open an issue on GitHub
- Check existing issues for similar problems
- Include relevant error messages and system information
- Reach out in Matrix [Invite link](https://matrix.to/#/#dev:matrix.cashu.space)

## Contributing

1. Create a feature branch
2. Make your changes
3. Run tests and formatting
4. Submit a pull request
5. Wait for review and address feedback

## Backporting Changes

CDK uses an automated backport bot to help maintain stable release branches. This section explains how the backport process works.

### How the Backport Bot Works

The backport bot creates pull requests to backport merged changes from `main` to stable release branches. **You control which branches to backport to by adding labels to your PR.**

**Available Target Branches:**
- `v0.10.x`
- `v0.11.x`
- `v0.12.x`
- `v0.13.x`

### Using Backport Labels

To backport a PR to specific stable branches, add labels to your PR **before or after merging**:

**Label Format:**
- `backport v0.13.x` - backports to v0.13.x branch
- `backport v0.12.x` - backports to v0.12.x branch
- Add multiple labels to backport to multiple branches

**Example Workflow:**
1. Create and merge your PR to `main`
2. Add label `backport v0.13.x` to the PR
3. The bot automatically creates a backport PR for the v0.13.x branch
4. Review and merge the backport PR
5. Repeat for other branches as needed

**When to Add Labels:**
- Add labels before merging - backport PRs are created automatically on merge
- Add labels after merging - backport PRs are created when you add the label
- You can add multiple backport labels at once

### When Backports Fail

Sometimes the backport bot cannot automatically create a backport PR due to merge conflicts or other issues. When this happens:

1. The bot automatically creates a GitHub issue labeled with `backport`
2. The issue will contain details about the original PR and which branch(es) failed
3. You'll need to manually create the backport PR for the failed branch

**Manual Backporting Process:**
```bash
# Checkout the target stable branch
git checkout v0.13.x
git pull origin v0.13.x

# Create a new branch for the backport
git checkout -b backport-pr-NUMBER-to-v0.13.x

# Cherry-pick the commits from the original PR
git cherry-pick COMMIT_HASH

# Resolve any conflicts if they occur
# Then push and create a PR
git push origin backport-pr-NUMBER-to-v0.13.x
```

### Best Practices for Backporting

1. **Label Appropriately:** Only add backport labels for changes that should be in stable branches
2. **Keep PRs Focused:** Smaller, focused PRs are easier to backport automatically
3. **Review Backport PRs:** Always review automatically created backport PRs to ensure they're appropriate
4. **Test Backports:** Run tests on backport PRs just like regular PRs
5. **Address Conflicts Promptly:** If a backport fails, address it promptly or close the issue with an explanation

### When NOT to Backport

Not all changes should be backported to stable branches. **Don't add backport labels** for:
- Breaking API changes
- New features that aren't needed in older versions
- Changes that don't apply to older version branches
- Large refactorings
- Experimental or unstable features

If a backport isn't appropriate, simply don't add the backport label to the PR.

## CI & Infrastructure

CDK uses a specialized self-hosted infrastructure for CI/CD, specifically for fuzzing and integration tests.

### Self-Hosted Runners

Our infrastructure is defined in the [cdk-infra](https://github.com/thesimplekid/cdk-infra) repository. It utilizes a "warm pool" of ephemeral NixOS containers to provide reproducible, isolated, and high-performance runners.

**Key Features:**
- **Ephemeral:** Each job runs in a fresh, ephemeral NixOS container that is destroyed immediately after use.
- **Warm Pool:** Containers are pre-provisioned to ensure instant job pickup.
- **Nix Native:** Fully supports Nix builds and caching.
- **Isolation:** Runners are network-isolated for security.

### Architecture

The system consists of:
- **Runners:** Two dedicated hosts (`cdk-runner-01` and `cdk-runner-02`) running NixOS.
- **Controller:** A custom Rust controller manages the lifecycle of the containers, monitoring the repository for queued jobs and maintaining the warm pool.

For more details on the infrastructure implementation, deployment, and management, please refer to the [cdk-infra repository](https://github.com/thesimplekid/cdk-infra).

## Additional Resources

- [Nix Documentation](https://nixos.org/manual/nix/stable/)
- [Contributing Guidelines](CODE_STYLE.md)

## License

Refer to the LICENSE file in the repository for terms of use and distribution.
