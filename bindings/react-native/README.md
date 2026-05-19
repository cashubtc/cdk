# @cashudevkit/react-native

React Native bindings for the [Cashu Development Kit (CDK)](https://github.com/cashubtc/cdk), powered by [Nitro Modules](https://github.com/mrousavy/nitro).

Provides native Cashu protocol cryptography (DHKE blinding, NUT-10 P2PK, NUT-13 deterministic secrets) backed by the CDK Rust library, with prebuilt binaries for iOS and Android — no Rust toolchain required.

## Installation

Install directly from the git repository:

```sh
# npm
npm install github:cashubtc/cdk-nitro

# yarn
yarn add cashubtc/cdk-nitro

# with a specific release tag
npm install github:cashubtc/cdk-nitro#v0.1.0
```

### Peer dependencies

```sh
npm install react-native-nitro-modules
```

### iOS

```sh
cd ios && pod install
```

### Android

No additional setup — the prebuilt `.so` libraries are included in the package and linked automatically via CMake.

## Usage

```typescript
import { OutputDataCreator } from '@cashudevkit/react-native';

// Create an instance of the native output data creator
const creator = new OutputDataCreator();

// Create a random blinded message
const output = creator.createSingleRandomData(64, '009a1f293253e41e');
console.log(output.blindedSecret); // hex-encoded blinded point (B_)
console.log(output.blindingFactor); // hex-encoded blinding factor (r)
console.log(output.secret);         // the secret used for blinding

// Create a P2PK locked output
const p2pkOutput = creator.createSingleP2PKData(
  {
    pubkey: '02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc',
    numSigs: 1,
    sigFlag: 'SigInputs',
  },
  64,
  '009a1f293253e41e',
);

// Create deterministic outputs (NUT-13)
const seed = new ArrayBuffer(64); // your BIP32 seed
const deterministicOutput = creator.createSingleDeterministicData(
  64,
  seed,
  0, // counter
  '009a1f293253e41e',
);
```

## API

### `OutputDataCreator`

#### Random outputs

- **`createSingleRandomData(amount, keysetId)`** — Create a single blinded message with an ephemeral random secret.
- **`createRandomData(amount, keysetId, keys, customSplit?)`** — Create multiple blinded messages, splitting the amount across denominations.

#### P2PK outputs (NUT-10)

- **`createSingleP2PKData(p2pk, amount, keysetId)`** — Create a single blinded message locked to a public key.
- **`createP2PKData(p2pk, amount, keysetId, keys, customSplit?)`** — Create multiple P2PK-locked blinded messages.

#### Deterministic outputs (NUT-13)

- **`createSingleDeterministicData(amount, seed, counter, keysetId)`** — Create a single deterministic blinded message from a BIP32 seed and counter.
- **`createDeterministicData(amount, seed, counter, keysetId, keys, customSplit?)`** — Create multiple deterministic blinded messages.

### Types

```typescript
interface OutputData {
  amount: number;
  keysetId: string;
  blindedSecret: string;  // hex-encoded compressed point (B_)
  blindingFactor: string;  // hex-encoded secret key (r)
  secret: string;
}

interface P2PKOptions {
  pubkey: string;            // recipient pubkey (33-byte compressed hex)
  additionalPubkeys?: string[];
  numSigs?: number;
  locktime?: number;
  refundPubkeys?: string[];
  numSigsRefund?: number;    // refund multisig threshold (2+ to require multiple)
  sigFlag?: string;          // 'SigInputs' | 'SigAll'
}

interface KeyEntry {
  amount: number;
  pubkey: string;
}
```

## Supported platforms

| Platform | Architecture | Status |
|----------|-------------|--------|
| iOS | arm64 (device) | Prebuilt |
| iOS | arm64 (simulator) | Prebuilt |
| Android | arm64-v8a | Prebuilt |
| Android | armeabi-v7a | Prebuilt |
| Android | x86_64 | Prebuilt |

## Building from source

If you need to build the native library yourself (e.g. for a target not listed above):

```sh
# Requires: Rust toolchain, appropriate cross-compilation targets

# iOS
cd rust && ./build-ios.sh

# Android (requires Android NDK and cargo-ndk)
cd rust && ./build-android.sh
```

## License

MIT
