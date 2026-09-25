import { NitroModules } from 'react-native-nitro-modules';

import type { CashuCrypto } from './generated/CashuCrypto.nitro';
import { withNativeErrors } from './generated/CashuCryptoErrors';

export type {
  BlindPair,
  BlindedOutput,
  CashuCrypto,
  DeterministicOutputFactory,
  DleqProof,
  KeyEntry,
  P2pkOptions,
  SigFlag,
} from './generated/CashuCrypto.nitro';

export {
  CashuFfiError,
  toNativeError,
  withNativeErrors,
} from './generated/CashuCryptoErrors';
export type { CashuFfiErrorKind } from './generated/CashuCryptoErrors';

let instance: CashuCrypto | undefined;

/**
 * The native module, created once per app.
 *
 * @throws If the native library is not linked into the running app.
 */
export function getCashuCrypto(): CashuCrypto {
  if (instance === undefined) {
    instance = NitroModules.createHybridObject<CashuCrypto>('CashuCrypto');
  }
  return instance;
}

/**
 * Whether the native module is present.
 *
 * Lets a caller keep a pure TypeScript fallback for JavaScript runtimes that
 * are not React Native.
 */
export function isNativeAvailable(): boolean {
  try {
    getCashuCrypto();
    return true;
  } catch {
    return false;
  }
}

/** Run `call` against the native module, raising typed errors. */
export function withCashuCrypto<T>(call: (crypto: CashuCrypto) => T): T {
  return withNativeErrors(() => call(getCashuCrypto()));
}
