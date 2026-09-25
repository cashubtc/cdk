# CDK Kotlin Bindings

Kotlin/JVM and Android bindings for the [Cashu Development Kit](https://github.com/cashubtc/cdk), generated via [UniFFI](https://mozilla.github.io/uniffi-rs/).

## Module Architecture

```
cdk-jvm       Generated Kotlin bindings + JNA native loading (not published)
cdk-android   Self-contained Android library (published to Maven Central)
```

Both modules use [JNA](https://github.com/java-native-access/jna) to load the
native Rust library. `cdk-jvm` holds the generated Kotlin sources and is used
for desktop development and tests; it is **not** published. `cdk-android`
compiles those same sources directly into its AAR and bundles pre-built `.so`
files for the `arm64-v8a` and `x86_64` Android ABIs, so the published artifact
is self-contained.

Only Android is published to Maven Central. Desktop JVM users build from source
(see [Building from source](#building-from-source)). This keeps the published
footprint within Maven Central's per-organization publishing limits.

## Maven Artifacts

Published under `org.cashudevkit`:

| Artifact | Description |
|---|---|
| `cdk-android` | Self-contained Android library (bindings + `arm64-v8a`/`x86_64` jniLibs) |

Releases up to and including 0.17.x also published `cdk-jvm` and
`cdk-jvm-natives` for desktop JVM use. Those coordinates remain available for
their existing versions but are no longer published for new releases.

## Installation

### Android

```kotlin
dependencies {
    implementation("org.cashudevkit:cdk-android:VERSION")
}
```

### Desktop JVM

Not published to Maven Central. Build the bindings and native library from
source (see [Building from source](#building-from-source)) and depend on the
local `cdk-jvm` module.

## Quick Start

```kotlin
import org.cashudevkit.*
import kotlinx.coroutines.runBlocking

fun main() = runBlocking {
    val mnemonic = generateMnemonic()

    val wallet = Wallet(
        mintUrl = "https://testnut.cashudevkit.org",
        unit = CurrencyUnit.Sat,
        mnemonic = mnemonic,
        store = WalletStore.Sqlite(path = "wallet.sqlite"),
        config = WalletConfig(targetProofCount = null),
    )

    // Request a mint quote
    val quote = wallet.mintQuote(
        paymentMethod = PaymentMethod.Bolt11,
        amount = Amount(value = 100UL),
        description = null,
        extra = null,
    )

    println("Pay this invoice: ${quote.request}")

    // After payment settles, mint the tokens
    val proofs = wallet.mint(
        quoteId = quote.id,
        amountSplitTarget = SplitTarget.None,
        spendingConditions = null,
    )

    val balance = wallet.totalBalance()
    println("Balance: ${balance.value} sats")

    wallet.close()
}
```

## Building from source

The native library and the generated Kotlin sources are built in the
[CDK monorepo](https://github.com/cashubtc/cdk), not here.

```bash
git clone https://github.com/cashubtc/cdk
cd cdk
just binding-kotlin
```

`just binding-kotlin` runs `nix build .#kotlin-bindings` and copies the generated
sources and the host native library into `bindings/kotlin/cdk-jvm/src/main/`. It
needs [nix](https://nixos.org/download) with flakes enabled and
[just](https://github.com/casey/just). Nix supplies the Rust toolchain pinned in
`rust-toolchain.toml`, so a separate rustup install is not required.

Run the tests from the monorepo root with `just test-kotlin`. That runs the JVM
module only (`-PcdkJvmOnly=true`), so it needs a JDK but no Android SDK.

The published Android libraries are cross-compiled in the CDK monorepo's
`.#kotlin-build` Nix devshell under the `release-ffi` profile, against the
workspace `Cargo.lock`.

The cdk-kotlin repository carries build artifacts, not sources: generated Kotlin
sources, the Gradle projects and the Android libraries under
`cdk-android/src/main/jniLibs/`, but no Rust crate. Desktop JVM users build here
in the monorepo, where the FFI crate and the lockfile live. The libraries are
committed as well as published to Maven Central, so a nightly tag, which skips
the Maven publish, still carries them.

## CI/CD — Publishing Workflow

The `kotlin-publish.yml` workflow (in the CDK monorepo) builds the Android
native binaries, syncs sources to `cdk-kotlin`, publishes to Maven Central, and
creates a tagged GitHub release. The `cdk-android` artifact is uploaded in one
direct Central Portal deployment with redundant checksum files removed. The following secrets and variables must be configured
in the **CDK monorepo** repository settings (Settings → Secrets and variables →
Actions).

### Secrets

| Name | Purpose |
|---|---|
| `FFI_DEPLOY_KEY` | Personal access token (PAT) with `repo` scope on the FFI target repos (`cdk-dart`, `cdk-kotlin`, `cdk-swift`). Used to clone, push, and create releases. Shared across all FFI publish workflows. |
| `SONATYPE_USERNAME` | Maven Central Portal user-token username for publishing. |
| `SONATYPE_PASSWORD` | Maven Central Portal user-token password. |
| `SIGNING_KEY` | ASCII-armored GPG private key for signing Maven artifacts. |
| `SIGNING_PASSWORD` | Passphrase for the GPG signing key. |

#### How to create the PAT

1. Go to **GitHub → Settings → Developer settings → Personal access tokens → Fine-grained tokens**.
2. Create a token scoped to the `cdk-dart`, `cdk-kotlin`, and `cdk-swift` repositories with **Contents** (read/write) and **Metadata** (read) permissions.
3. Add it as a repository secret named `FFI_DEPLOY_KEY` in the monorepo.

#### Maven Central (Sonatype) setup

1. Register at [central.sonatype.com](https://central.sonatype.com/) and claim the `org.cashudevkit` namespace.
2. Generate a user token under **Account → User Token**.
3. Add the username and password as `SONATYPE_USERNAME` and `SONATYPE_PASSWORD` secrets.

#### GPG signing key

1. Generate a key: `gpg --full-generate-key` (RSA 4096, no expiry is fine for CI).
2. Export the ASCII-armored private key: `gpg --armor --export-secret-keys <KEY_ID>`.
3. Add the full output as the `SIGNING_KEY` secret and the passphrase as `SIGNING_PASSWORD`.

### Variables

| Name | Purpose | Example |
|---|---|---|
| `CDK_KOTLIN_REPO` | Owner/repo of the target Kotlin package repository. | `cashubtc/cdk-kotlin` |

Set this under **Settings → Secrets and variables → Actions → Variables**.
