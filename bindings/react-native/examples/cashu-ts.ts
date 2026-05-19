/**
 * Using @cashudevkit/react-native with cashu-ts.
 *
 * Consumer app deps assumed: @cashu/cashu-ts (>= 5.0.0-rc.0),
 * react-native-nitro-modules (peer), and @cashudevkit/react-native.
 *
 * The native OutputDataCreator offloads DHKE blinding, NUT-10 P2PK, and NUT-13
 * deterministic secret derivation to the CDK Rust library. Inject it through the
 * Wallet `outputDataCreator` option; the rest is plain cashu-ts.
 */
import { Mint, Wallet } from '@cashu/cashu-ts';
// The native singleton. Import it from the /native entry, which loads the Nitro
// runtime and therefore only works inside a React Native app.
import { cashuOutputDataCreator } from '@cashudevkit/react-native/native';

export async function receiveWithNativeCrypto(
  mintUrl: string,
  bip39seed: Uint8Array,
  token: string,
) {
  const mint = new Mint(mintUrl);
  const wallet = new Wallet(mint, {
    unit: 'sat',
    // Deterministic (NUT-13) secrets are derived natively from this seed.
    bip39seed,
    // Native DHKE blinding and secret construction.
    outputDataCreator: cashuOutputDataCreator,
  });

  // Every output (swap/receive/mint/send) is now built by the native module.
  return wallet.receive(token);
}

// To inject a custom or stubbed native instance instead of the default
// singleton (for example in unit tests), wrap it with the pure factory from the
// package root, which pulls in no Nitro runtime:
//
//   import { createCashuOutputDataCreator } from '@cashudevkit/react-native';
//   const outputDataCreator = createCashuOutputDataCreator(myNativeModule);
