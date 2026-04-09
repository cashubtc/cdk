# Development guide

Technical reference for **working on the CDK codebase**: architecture, environments, build/test/debug workflows, databases, CI, and releases.

**New to the project?** Start with [CONTRIBUTING.md](CONTRIBUTING.md#contributing-to-cdk). This document is the **deep** reference.

## 1. Project overview

### Workspace and MSRV

- **Workspace:** Rust edition **2021**, members include `crates/*` and `bindings/*/rust` (see root `Cargo.toml`).
- **MSRV:** **`rust-version`** in **`[workspace.package]`** in the root **[Cargo.toml](Cargo.toml)** (source of truth). CI also runs **MSRV** checks via Nix flake `msrv-*` checks. Default **developer** toolchain pins are in **`rust-toolchain.toml`**.

### High-level architecture

```text
                         ┌─────────────┐     ┌────────────────┐
                         │  cdk-cli    │     │ FFI / bindings │
                         └──────┬──────┘     └───────┬────────┘
                                │                    │
                                └────────┬───────────┘
                                         ▼
                              ┌──────────────────────┐
                              │ cashu  ·  cdk-common │
                              │         + cdk          │
                              └───────────┬────────────┘
                                          │
        ┌─────────────────────────────────┼─────────────────────────────────┐
        │                                 │                                 │
        ▼                                 ▼                                 ▼
 ┌──────────────┐              ┌─────────────────┐              ┌───────────────┐
 │cdk-http-     │   HTTP/WS     │ cdk-axum        │              │ cdk-mintd     │
 │client        │──────────────►│ (mint HTTP)     │              │ (daemon)      │
 └──────────────┘              └────────┬────────┘              └───────┬───────┘
        ▲                               │                               │
        │                               ▼                               │
        │                      ┌────────────────┐                       │
        │                      │ Mint logic in  │◄──────────────────────┘
        │                      │ cdk + DB I/O   │
        │                      └───────┬────────┘
        │                              │
        │              ┌───────────────┼───────────────┐
        │              ▼               ▼               ▼
        │      ┌──────────────┐ ┌─────────────┐ ┌──────────────────────────┐
        │      │ cdk-sqlite / │ │ cdk-supabase│ │ MintPayment-> CLN, LND,  │
        │      │ cdk-postgres │ │             │ │ LNbits, LDK-node,      │
        │      │ + cdk-sql-   │ │             │ │ cdk-fake-wallet        │
        │      │   common     │ │             │ └──────────────────────────┘
        │      └──────────────┘ └─────────────┘
        │      Wallet-side Redb: cdk-redb (via cdk)
        └────── proofs / quotes / errors (JSON, WS)
```

### Data flow (wallet ↔ HTTP ↔ mint ↔ Lightning)

```text
  Wallet / cdk-cli
        │
        │  NUT requests (mint, melt, swap, …)
        ▼
  cdk-http-client ── HTTP / WebSocket ──► cdk-axum (mint server)
                                                    │
                                                    │ route to handlers / sagas
                                                    ▼
                                             Mint core (cdk)
                                               │         │
                         persist state         │         │ BOLT11 / BOLT12,
                         (SQLite / Postgres / │         │ invoices, payments
                         …)                    │         ▼
                         ▼                     │    Lightning backend
                    Database ◄────────────────┘         │
                         ▲                              │
                         └──────── payment outcome ────┘

  Responses: JSON / WebSocket events back through cdk-axum-> client-> wallet
            (proofs, quotes, errors)
```

### How crates depend on each other

- **`cashu`** - Protocol types, crypto, NUT modules. No dependency on `cdk`.
- **`cdk-common`** - Traits (`MintDatabase`, `WalletDatabase`, `MintPayment`, …), shared errors, optional pub/sub. Depends on **`cashu`**.
- **`cdk`** - Wallet and mint **business logic**; depends on **`cdk-common`**, **`cashu`**, **`cdk-signatory`** (signing), etc.
- **`cdk-sql-common`** - Shared SQL migrations and query helpers; **not** a storage backend you choose by name—**`cdk-sqlite`** and **`cdk-postgres`** embed it.
- **Lightning backends** implement `MintPayment` (and related traits) from **`cdk-common`**; **`cdk-mintd`** wires one backend at runtime.
- **`cdk-axum`** - HTTP/WebSocket layer for the mint; uses **`cdk`** mint types and **`cdk-common`**.
- **`cdk-http-client`** - Wallet-side HTTP/WebSocket client.

### Storage layer architecture

- **Traits** live in **`cdk-common`** (`MintDatabase`, `WalletDatabase`, …).
- **SQL backends:** **`cdk-sql-common`** holds SQL migrations (`src/mint/migrations/sqlite|postgres`, `src/wallet/migrations/...`, auth migrations) and shared logic. **`cdk-sqlite`** / **`cdk-postgres`** provide concrete pools and implement the traits.
- **`cdk-redb`** - Embedded **wallet** storage (Redb), not routed through the SQL migration tree in the same way.
- **`cdk-supabase`** - Cloud-oriented storage integration.

On startup, SQL backends **run embedded migrations** (see [§5 Database development](#5-database-development)); there is typically **no separate `migrate` CLI** for the wallet CLI in-tree.

---

## 2. Development environment setup

### Option 1: Nix flakes (recommended)

CDK uses **Nix flakes** for reproducible toolchains and shells.

```bash
# Install Nix (pick one)
# - Recommended: https://github.com/DeterminateSystems/nix-installer
# - Official: https://nixos.org/download.html

# If needed, enable flakes in ~/.config/nix/nix.conf or /etc/nix/nix.conf:
#   experimental-features = nix-command flakes

nix develop              # default “stable” shell: Rust, PostgreSQL helpers, protobuf, …
nix develop .#regtest    # + bitcoind, CLN, LND, mprocs (full local stack)
nix develop .#integration # for Docker-heavy / auth tests (Keycloak, etc.)
nix develop .#ffi        # Python + UniFFI for cdk-ffi
nix develop .#bindings   # Dart/Kotlin/Swift binding builds (see CI)
```

**Shell overview**

| Shell            | Command                         | Notes                                        |
| ---------------- | ------------------------------- | -------------------------------------------- |
| Stable (default) | `nix develop`                   | Daily Rust + **Postgres helpers** + `protoc` |
| Regtest          | `nix develop .#regtest`         | Bitcoin + Lightning + mint stacks            |
| Nightly          | `nix develop .#nightly`         | Nightly rustfmt / experiments                |
| nightly-regtest  | `nix develop .#nightly-regtest` | Nightly + regtest stack                      |
| Integration      | `nix develop .#integration`     | Regtest + **Docker**                         |
| MSRV             | `nix develop .#msrv`            | Minimum Rust toolchain                       |
| FFI              | `nix develop .#ffi`             | UniFFI / Python                              |
| bindings         | `nix develop .#bindings`        | Mobile bindings (see `justfile`)             |

### Option 2: Manual setup (no Nix)

1. **Rust:** [rustup](https://rustup.rs/), then match toolchain from **`rust-toolchain.toml`**.
2. **protobuf:** `protoc` for gRPC crates and `cdk-mintd` (e.g. `apt install protobuf-compiler`, `brew install protobuf`).
3. **PostgreSQL:** For `cdk-postgres` tests - local server or Docker; set `DATABASE_URL` as required by tests.
4. **SQLite:** Usually via the `libsqlite3-sys` / `sqlite` crate; on Ubuntu `libsqlite3-dev` if linking fails.
5. **Docker:** Optional; used for Keycloak auth integration tests (`misc/keycloak/docker-compose*.yml`) and some `just` workflows.

### PostgreSQL helpers (Nix shells)

Inside the default dev shell:

- `start-postgres` - init and start local Postgres (data in `.pg_data/`)
- `stop-postgres` - stop
- `pg-status` - status
- `pg-connect` - `psql` shell

### IDE setup

**Recommended:** **VS Code** or **Cursor** with **rust-analyzer**.

- **Extensions:** `rust-lang.rust-analyzer`, `tamasfe.even-better-toml`, and (optional) `nix-community.nix-ide` if you edit Nix.
- **Repo settings:** [.vscode/settings.json](.vscode/settings.json) enables clippy as the `rust-analyzer` check command with `-D warnings`. If checks are slow on save, switch `rust-analyzer.check.command` to `"check"` locally.

**Helix:** `.helix/` exists for Helix editor users.

---

## 3. Building the project

```bash
# Entire workspace
cargo build --workspace --all-targets
```

```bash
# or use just (from repo root)
just build
```

```bash
# Single crate
cargo build -p cdk-sqlite
```

```bash
# All features (may be slow; some combinations are feature-gated per crate)
cargo build --workspace --all-targets --all-features
```

```bash
# Release binaries
cargo build --release --bin cdk-cli
cargo build --release --bin cdk-mintd
```

### Nix flake outputs (without `cargo`)

```bash
nix build .#cdk-cli
./result/bin/cdk-cli --help

nix build .#cdk-mintd
```

```bash
# Static musl binaries (Linux x86_64; see flake for targets)
just build-static cdk-mintd-static
nix build .#cdk-mintd-static
```

Release automation: [static-build-publish.yml](.github/workflows/static-build-publish.yml).

### Common build issues

| Problem                                       | What to try                                                                                                                                           |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **`openssl-sys` / native TLS build failures** | Most HTTP stack uses **rustls**; if a native SSL crate appears, install `libssl-dev` / `openssl-devel` and `pkg-config` (Linux). On macOS, Xcode CLT. |
| **PostgreSQL not found (pg tests)**           | Start Postgres (`start-postgres` in Nix shell) or point `DATABASE_URL` at your instance.                                                              |
| **Nix builds slow / cache misses**            | Cachix `cashudevkit` is used in CI; ensure substituters are trusted. Run `nix flake update` when inputs change.                                       |
| **`protoc` not found**                        | Install `protobuf-compiler` / `brew install protobuf`.                                                                                                |
| **Linking on macOS**                          | Ensure Xcode Command Line Tools are installed.                                                                                                        |

---

## 4. Testing strategy

### Unit tests

```bash
# All library tests (excludes cdk-postgres - needs DB)
cargo test --lib --workspace --exclude cdk-postgres
```

```bash
just test    # same + selective integration tests (see justfile)
```

```bash
# One crate
cargo test -p cdk-sqlite
```

```bash
# Feature-gated crate (example)
cargo test -p cdk --features wallet
```

### Regtest stack (Bitcoin + Lightning + mints)

For full local integration (not only Rust unit tests):

```bash
nix develop .#regtest
just regtest
```

See **[REGTEST_GUIDE.md](REGTEST_GUIDE.md)** for topology, mprocs, and workflows.

### Integration tests

Integration tests live in **`crates/cdk-integration-tests/`**. Some suites use **`just`** (often with **`nix develop .#regtest`**) and a prebuilt **nextest archive** in CI (`CDK_ITEST_ARCHIVE`).

```bash
nix develop .#regtest
just itest SQLITE          # or POSTGRES, REDB, MEMORY, … - see justfile
```

```bash
docker compose -f misc/keycloak/docker-compose-recover.yml up -d   # auth-related tests
# … run targeted tests from justfile (e.g. fake-auth-mint-itest)
```

**Pure integration tests** (memory / sqlite / redb):

```bash
just test-pure memory
```

**Postgres**

```bash
start-postgres   # in Nix shell
cargo test -p cdk-postgres
```

### Test organization

- **Unit tests:** `#[cfg(test)]` in `src/` or `tests/*.rs` per crate.
- **Integration tests:** `crates/cdk-integration-tests/tests/` and harness binaries.
- **Fixtures / helpers:** `crates/cdk/src/test_helpers/`, `crates/cdk-integration-tests/src/`, etc. - follow existing patterns.

### Testing practices

- **Mock Lightning** with **`cdk-fake-wallet`** in tests where possible.
- **Isolate DB state** - use temp dirs or in-memory SQLite when the test allows.
- **Run `cargo test --doc`** for doctests where relevant (`just test-units` includes doc tests).

### Mutation testing

```bash
cargo mutants
cargo mutants --file crates/cashu/src/amount.rs
```

See `.cargo/mutants.toml` and [mutation-testing-weekly.yml](.github/workflows/mutation-testing-weekly.yml).

### Common `just` recipes

```bash
just build
just test
just quick-check
just final-check
just format
just clippy
just itest SQLITE
```

Run `just` with no arguments to list all recipes.

---

## 5. Database development

### Where migrations live

- **SQL migrations:** `crates/cdk-sql-common/src/mint/migrations/` and `.../wallet/migrations/`, with subdirs **`sqlite/`** and **`postgres/`** (and auth-specific trees under `mint/auth/migrations/`).
- **Build:** `crates/cdk-sql-common/build.rs` discovers `migrations/` directories and embeds SQL at compile time.

### Naming

Use **`YYYYMMDDHHMMSS_short_description.sql`** (see existing files). Keep **SQLite and Postgres** in sync when both backends must change.

### Applying migrations

Applications (**`cdk-mintd`**, wallet DB code in **`cdk-sqlite`**, etc.) run the **embedded migration runner** when opening a pool - there is **no** separate `cargo run -p cdk-cli -- migrate` in this repository. If you add tooling, document it in the crate README.

### Creating a new migration

1. Add SQL under **`sqlite/`** and **`postgres/`** as needed.
2. Optionally scaffold a timestamp with:

   ```bash
   just new-migration mint my_change_name
   ```

   **Verify** the path matches how existing migrations are laid out (`sqlite/` / `postgres/`). If the recipe places a file at the wrong level, create files manually beside existing migrations.

3. Rebuild the crate that depends on `cdk-sql-common` to regenerate embedded includes.

### Migration SQL

- Prefer **database-agnostic** SQL where possible; otherwise maintain two files.
- Test **upgrade** paths; down migrations are not always modeled - follow project conventions in existing files.

---

## 6. Running examples

Examples are defined on the **`cdk`** crate:

```bash
cargo run -p cdk --example wallet --features wallet
```

```bash
cargo run -p cdk --example mint-token --features wallet
```

```bash
cargo run -p cdk --example receive-token --features wallet
```

List example sources:

```bash
ls crates/cdk/examples/
```

**Representative examples**

| Example                                     | `required-features` | What it demonstrates        |
| ------------------------------------------- | ------------------- | --------------------------- |
| `wallet`                                    | `wallet`            | Basic wallet usage          |
| `mint-token`, `receive-token`, `melt-token` | `wallet`            | Mint / receive / melt flows |
| `batch-mint`                                | `wallet`            | Batch minting               |
| `auth_wallet`                               | `wallet`            | Auth wallet patterns        |
| `p2pk`                                      | `wallet`            | P2PK                        |
| `nostr_backup`                              | `wallet` + `nostr`  | Nostr backup                |
| `npubcash`, `multimint-npubcash`            | `npubcash`          | npub.cash                   |
| `bip353`, `resolve_human_readable`          | `wallet`, `bip353`  | BIP-353 resolution          |

**Other crates:** e.g. `crates/cdk-npubcash/examples/`, `crates/cashu/examples/` - run with `-p cdk-npubcash` / `-p cashu`.

---

## 7. Docker development

### Compose stacks (repo root)

- **`docker-compose.yaml`** - Mint + Prometheus + Grafana + related services (see comments in file).
- **`docker-compose.postgres.yaml`**, **`docker-compose.ldk-node.yaml`**, **`docker-compose.tor.yaml`** - variant stacks.
- **`misc/keycloak/docker-compose.yml`** - Keycloak for auth integration tests.

```bash
docker compose up
```

(Use `--profile` or `-f` as documented in `crates/cdk-mintd/README.md` for LDK variants.)

### Building images

**CI publishes** static images to Docker Hub (`cashubtc/mintd`) - see [`docker-publish.yml`](.github/workflows/docker-publish.yml): builds **`nix` targets** `cdk-mintd-static` / `cdk-mintd-ldk-static`, copies binaries into **`Dockerfile.static`**, multi-arch build.

**Local Dockerfile (legacy Nix-in-Docker):**

```bash
docker build -f Dockerfile -t cdk-mintd:local .
```

Prefer **`nix build .#cdk-mintd-static`** + `Dockerfile.static` for images that match releases.

---

## 8. Debugging

### `println!` / `dbg!`

```rust
dbg!(&some_value);
tracing::debug!("{:#?}", some_struct);
```

Avoid committing noisy `println!` in non-test code; prefer **`tracing`** (see below).

### Debuggers

```bash
rust-lldb target/debug/cdk-cli
# or
rust-gdb target/debug/cdk-cli
```

### Logging (`tracing`)

CDK uses **`tracing`** (see [AGENTS.md](AGENTS.md) for macro style).

```bash
RUST_LOG=debug cargo run --bin cdk-cli -- …
RUST_LOG=debug cargo test -p cdk -- --nocapture
```

Use **`RUST_LOG=trace`** sparingly (very verbose).

### Common scenarios

| Symptom                | Where to look                                                            |
| ---------------------- | ------------------------------------------------------------------------ |
| DB connection failures | `DATABASE_URL`, Postgres running, `pg-status`, logs                      |
| Lightning timeouts     | Backend config, `cdk-fake-wallet` vs real node, regtest (`just regtest`) |
| Proof / crypto errors  | `cashu` crate, keyset state, `cdk` mint verification paths               |

---

## 9. Performance profiling

### Flamegraph

```bash
cargo install flamegraph
cargo flamegraph --bin cdk-mintd
```

### Benchmarks

```bash
cargo bench -p cashu
cargo bench -p cdk
```

Criterion benches exist where `[[bench]]` is declared (e.g. `crates/cashu`).

### Coverage

```bash
just coverage   # requires llvm-cov + Nix integration shell (see justfile)
```

---

## 10. Code style and linting

```bash
cargo fmt --all
cargo fmt --all -- --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo clippy --fix --workspace --all-targets -- -D warnings
```

```bash
typos
```

Workspace **lint rules** are in root `Cargo.toml` `[workspace.lints]` and `[workspace.lints.clippy]` (e.g. `unwrap_used = "deny"`). Full style guide: **[CODE_STYLE.md](CODE_STYLE.md)** - **mandatory** for contributors.

**PR tip:** CI uses **stable** rustfmt; nightly rustfmt may be applied by automated PRs - see [nightly-rustfmt.yml](.github/workflows/nightly-rustfmt.yml).

---

## 11. Documentation

### Build docs locally

```bash
cargo doc --workspace --no-deps --open
```

Strict docs checks run in CI (`flake` checks `doc-tests`, `strict-docs`).

### Writing rustdoc

````rust
/// Short summary line.
///
/// Longer explanation with examples:
///
/// # Examples
///
/// ```
/// use cdk::…;
/// ```
///
/// # Errors
///
/// Returns error when …
pub fn my_function() -> Result<(), Error> {
````

---

## 12. Release process

**Maintainer-oriented** - exact steps may evolve.

1. **Version bump** - `version` in `[workspace.package]` in root `Cargo.toml` and aligned crate versions (workspace uses `version.workspace = true` in most crates).
2. **CHANGELOG** - Update [CHANGELOG.md](CHANGELOG.md) for the release (see `[Unreleased]`-> release section).
3. **Tag** - Git tag (e.g. `v0.16.0`).
4. **crates.io** - Publish crates in dependency order (`cargo publish -p cashu`, then `cdk-common`, …) as maintainers do today.
5. **GitHub Releases** - Release notes + **static binaries** via [static-build-publish.yml](.github/workflows/static-build-publish.yml) (or manual `nix build` + checksums).
6. **Docker** - [docker-publish.yml](.github/workflows/docker-publish.yml) on `release: published` or `workflow_dispatch` with tag.

### Backporting to stable branches

Use GitHub labels like **`backport v0.13.x`** on the merged PR (see [backport.yml](.github/workflows/backport.yml)). The bot opens cherry-pick PRs; if it fails, an issue may be filed.

**Manual cherry-pick (example):**

```bash
git checkout v0.13.x
git pull origin v0.13.x
git checkout -b backport-pr-NUMBER-to-v0.13.x
git cherry-pick COMMIT_HASH
# resolve conflicts, test, then push and open PR
```

Do **not** backport breaking API changes, large refactors, or experimental work.

---

## 13. CI/CD pipeline

### Main workflows

| Workflow                                                                         | Role                                                                                                                                                                                                                                                                                                                                                                                 |
| -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **[ci.yml](.github/workflows/ci.yml)**                                           | Primary CI: **fmt**, **typos**, **quick-check** (`just quick-check`), **per-crate flake checks** (clippy + tests), **examples** (nix `example-*` checks), **Postgres tests**, **integration** (regtest, fake mint, pure, payment processor), **MSRV**, **WASM**, **auth** (Keycloak + Docker), **doc-tests**, **strict-docs**, **FFI**, **coverage**, **Dart/Kotlin/Swift bindings** |
| **[nutshell_itest.yml](.github/workflows/nutshell_itest.yml)**                   | Nutshell mint/wallet integration                                                                                                                                                                                                                                                                                                                                                     |
| **[docker-publish.yml](.github/workflows/docker-publish.yml)**                   | Docker Hub images on release / manual dispatch                                                                                                                                                                                                                                                                                                                                       |
| **[static-build-publish.yml](.github/workflows/static-build-publish.yml)**       | Static Linux binaries + `SHA256SUMS` on releases                                                                                                                                                                                                                                                                                                                                     |
| **[nightly-rustfmt.yml](.github/workflows/nightly-rustfmt.yml)**                 | Automated rustfmt PRs                                                                                                                                                                                                                                                                                                                                                                |
| **[mutation-testing-weekly.yml](.github/workflows/mutation-testing-weekly.yml)** | Weekly mutation tests                                                                                                                                                                                                                                                                                                                                                                |
| **[daily-flake-check.yml](.github/workflows/daily-flake-check.yml)**             | Flake health                                                                                                                                                                                                                                                                                                                                                                         |
| **[backport.yml](.github/workflows/backport.yml)**                               | Backport labels-> stable branches                                                                                                                                                                                                                                                                                                                                                    |

**Required checks:** Whatever branches protect on GitHub (typically `pre-commit-checks`, `quick-check`, matrix jobs, etc.). Exact required status names should match **Settings-> Branches** in the repo.

### Self-hosted runners

Heavy jobs use **self-hosted** runners and **Cachix** (`cashudevkit`). Infra details: [cdk-infra](https://github.com/thesimplekid/cdk-infra).

---

## 14. Troubleshooting guide

| Problem                                                         | Suggestion                                                                                                      |
| --------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| **`error: failed to run custom build command for openssl-sys`** | Install OpenSSL dev packages + `pkg-config`; or ensure no dependency forced OpenSSL when rustls is expected.    |
| **Database migration / schema errors**                          | Match Postgres/SQLite versions; wipe **dev** `.pg_data/` only if safe; check migrations under `cdk-sql-common`. |
| **Lightning backend timeout**                                   | Network, regtest topology, or use `cdk-fake-wallet` for tests.                                                  |
| **Nix command not found**                                       | Enter `nix develop`; regtest-only tools need `.#regtest`.                                                       |
| **`just itest` fails on macOS**                                 | See notes in `justfile` / flake about `nixpkgs` pin (`nixos-unstable` workaround).                              |

---

## 15. Architecture deep dives

### Cashu protocol flow (summary)

- **Mint** publishes keys and keyset info; **wallet** requests **blind signatures** for mints; **proofs** encode amounts and secrets; **melt** swaps proofs for Lightning payment.
- **DHKE**, **DLEQ** (where applicable), and **NUT** rules are implemented in **`cashu`** and orchestrated by **`cdk`**.

### Storage layer

- **`cdk-sql-common`** - Shared migrations and query modules; `build.rs` wires SQL into Rust.
- **Transactions** - Use pool APIs and existing `run_db_operation` patterns from `cdk-common` / SQL crates.

### Lightning integration

- Backends implement **`MintPayment`** from **`cdk-common`**; **`cdk-mintd`** selects one via config.
- **Quote lifecycle** - mint/melt quotes, invoice handling, and payment verification differ per backend; see each `cdk-*` crate and `cdk` sagas.

---

## 16. Adding a new crate

1. **Create** `crates/my-crate/` with `Cargo.toml` (`version.workspace = true`, `edition.workspace = true`, `rust-version.workspace = true`, `license.workspace = true` as in other crates).
2. **Workspace** - `crates/*` is a glob; new directories auto-join unless excluded.
3. **Implement** traits / APIs; add **tests** and **README.md**.
4. **Wire** dependencies - add to `[workspace.dependencies]` in root `Cargo.toml` if shared.
5. **Document** in the root README “Project structure” if user-facing.
6. **CI** - Nix flake may need a new `checks` entry for clippy/tests; follow existing patterns.

---

## 17. Security considerations

- **Secrets** - Never commit keys, mnemonics, or production URLs; use env vars and config files excluded from git.
- **Crypto** - Prefer **`cashu`** / **`cdk`** primitives; do not roll your own crypto.
- **Disclosure** - Report vulnerabilities per **SECURITY.md** (private email), not public issues.
- **Unsafe** - Workspace forbids **`unsafe_code`** (`forbid` in `Cargo.toml`).

---

## 18. Resources

| Resource    | Link                                        |
| ----------- | ------------------------------------------- |
| Cashu org   | https://github.com/cashubtc                 |
| NUTs        | https://github.com/cashubtc/nuts            |
| Matrix #dev | https://matrix.to/#/#dev:matrix.cashu.space |
| This repo   | https://github.com/cashubtc/cdk             |

---

## License

See [LICENSE](LICENSE).
