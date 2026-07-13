# CDK Kotlin Bindings

Kotlin/JVM and Android bindings for the [Cashu Development Kit](https://github.com/cashubtc/cdk), generated via [UniFFI](https://mozilla.github.io/uniffi-rs/).

## Module Architecture

```
cdk-jvm              Core Kotlin bindings + JNA native loading
cdk-jvm-natives      Combined desktop native library JAR
cdk-android          Android wrapper with 64-bit jniLibs
```

**Dependency graph:**

```
cdk-android ──api──> cdk-jvm
cdk-jvm-natives      (standalone — native binaries only)
```

`cdk-jvm` contains the generated Kotlin sources and uses [JNA](https://github.com/java-native-access/jna) to load the native Rust library at runtime. `cdk-jvm-natives` provides Linux x86-64, Linux ARM64, and macOS Apple Silicon libraries in one JAR; JNA selects the matching directory at runtime. `cdk-android` depends on `cdk-jvm` and bundles pre-built `.so` files for the `arm64-v8a` and `x86_64` Android ABIs.

## Maven Artifacts

All artifacts are published under `org.cashudevkit`:

| Artifact | Description |
|---|---|
| `cdk-jvm` | Kotlin bindings (required) |
| `cdk-jvm-natives` | Desktop native libs for Linux x86-64/ARM64 and macOS Apple Silicon |
| `cdk-android` | Android library (includes 64-bit device and emulator ABIs) |

Older releases used one `cdk-jvm-<platform>` coordinate per desktop target.
Starting with the first release containing this change, use `cdk-jvm-natives`
instead. The historical coordinates remain available for their existing versions.

## Installation

### JVM

```kotlin
dependencies {
    implementation("org.cashudevkit:cdk-jvm:VERSION")
    runtimeOnly("org.cashudevkit:cdk-jvm-natives:VERSION")
}
```

### Android

```kotlin
dependencies {
    implementation("org.cashudevkit:cdk-android:VERSION")
    // cdk-jvm is included transitively
}
```

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

## Building from Source

Requires Rust and the [just](https://github.com/casey/just) command runner.

```bash
# Generate Kotlin bindings and build native library
just binding-kotlin

# Run tests
just test-kotlin
```

## CI/CD — Publishing Workflow

The `kotlin-publish.yml` workflow (in the CDK monorepo) builds native binaries
for the supported JVM and Android platforms, syncs sources to `cdk-kotlin`,
publishes to Maven Central, and creates a tagged GitHub release. The three Maven
artifacts are uploaded in one direct Central Portal deployment with redundant
checksum files removed. The following secrets and variables must be configured
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
