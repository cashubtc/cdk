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
npm install react-native-nitro-modules @cashu/cashu-ts
```

`@cashu/cashu-ts` (>= 5.0.0-rc.0) is required: the adapter wraps native results
in cashu-ts `OutputData` instances so they can be injected wherever cashu-ts
expects an `OutputDataCreator`.

### iOS

```sh
cd ios && pod install
```

### Android

No additional setup — the prebuilt `.so` libraries are included in the package and linked automatically via CMake.

## Entry points

The package has two entry points:

- **`@cashudevkit/react-native`** (root) is free of native code and safe to
  import anywhere, including Node/Bun/Vitest. It exports the pure
  `CdkOutputDataCreator` adapter, the `createCashuOutputDataCreator` factory, and
  the types. Use it for typechecking and tests.
- **`@cashudevkit/react-native/native`** instantiates the native Nitro
  HybridObject and must only be loaded inside a React Native runtime. It exports
  the ready-to-use `OutputDataCreator` singleton and `cashuOutputDataCreator`.

## Usage

`OutputDataCreator` is an already-constructed native HybridObject singleton, so
call its methods directly (do not use `new`):

```typescript
import { OutputDataCreator } from '@cashudevkit/react-native/native';

// Create a random blinded message
const output = OutputDataCreator.createSingleRandomData(64, '009a1f293253e41e');
console.log(output.blindedSecret); // hex-encoded blinded point (B_)
console.log(output.blindingFactor); // hex-encoded blinding factor (r)
console.log(output.secret);         // the secret used for blinding

// Create a P2PK locked output
const p2pkOutput = OutputDataCreator.createSingleP2PKData(
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
const deterministicOutput = OutputDataCreator.createSingleDeterministicData(
  64,
  seed,
  0, // counter
  '009a1f293253e41e',
);
```

The raw `OutputDataCreator` returns flat native results. To use the native
crypto wherever cashu-ts expects an `OutputDataCreator`, use
`cashuOutputDataCreator`, which wraps each result in a cashu-ts `OutputData`
instance. The two sections below show it wired into cashu-ts and into coco.

## Usage with cashu-ts

Inject `cashuOutputDataCreator` through the `Wallet` `outputDataCreator` option.
Everything else stays plain cashu-ts; blinding and NUT-13 derivation now run in
native code. Full example: [`examples/cashu-ts.ts`](./examples/cashu-ts.ts).

```typescript
import { Mint, Wallet } from '@cashu/cashu-ts';
import { cashuOutputDataCreator } from '@cashudevkit/react-native/native';

const wallet = new Wallet(new Mint(mintUrl), {
  unit: 'sat',
  bip39seed, // Uint8Array; deterministic (NUT-13) secrets are derived natively
  outputDataCreator: cashuOutputDataCreator,
});

const proofs = await wallet.receive(token);
```

To inject a custom or stubbed native instance instead of the default singleton
(for example in unit tests), wrap it with the factory from the root entry, which
imports no native code:

```typescript
import { createCashuOutputDataCreator } from '@cashudevkit/react-native';

const outputDataCreator = createCashuOutputDataCreator(myNativeModule);
```

## Usage with coco

coco exposes `outputDataCreator` as a first-class `CocoConfig` field, so the
native adapter is injected once and reused across every output-producing flow
(mint, receive, send, restore). Full examples:
[`examples/coco.ts`](./examples/coco.ts) and
[`examples/coco-react.tsx`](./examples/coco-react.tsx).

Headless (`@cashu/coco-core`):

```typescript
import { initializeCoco } from '@cashu/coco-core';
import { SqliteRepositories } from '@cashu/coco-expo-sqlite';
import { openDatabaseAsync } from 'expo-sqlite';
import { cashuOutputDataCreator } from '@cashudevkit/react-native/native';

const repo = new SqliteRepositories({ database: await openDatabaseAsync('coco.db') });

const coco = await initializeCoco({
  repo,
  seedGetter, // () => Promise<Uint8Array>, the app's 64-byte BIP-39 seed
  outputDataCreator: cashuOutputDataCreator,
});
```

React (`@cashu/coco-react`) — pass the same config to the provider:

```tsx
import { CocoCashuProvider } from '@cashu/coco-react';
import { cashuOutputDataCreator } from '@cashudevkit/react-native/native';

<CocoCashuProvider
  config={{ repo, seedGetter, outputDataCreator: cashuOutputDataCreator }}
>
  <App />
</CocoCashuProvider>;
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
| iOS | x86_64 (simulator) | Prebuilt |
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
