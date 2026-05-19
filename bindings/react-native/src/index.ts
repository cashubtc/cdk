/**
 * Package root. This entry is free of native (Nitro) code, so it is safe to
 * import in any environment, including Node/Bun/Vitest, for typechecking and
 * tests. To use the native-backed singleton on a device, import from
 * `@cashudevkit/react-native/native`.
 */
import { CdkOutputDataCreator } from './CdkOutputDataCreator';
import type { NativeOutputDataCreator } from './CdkOutputDataCreator';

export type {
  OutputData,
  KeyEntry,
  P2PKOptions,
  OutputDataCreator as OutputDataCreatorSpec,
} from './specs/OutputDataCreator.nitro';

export { CdkOutputDataCreator } from './CdkOutputDataCreator';
export type { NativeOutputDataCreator } from './CdkOutputDataCreator';

/**
 * Wrap a native OutputDataCreator (the Nitro HybridObject from
 * `@cashudevkit/react-native/native`, or a compatible stand-in for tests) in the
 * cashu-ts-compatible adapter.
 */
export function createCashuOutputDataCreator(
  native: NativeOutputDataCreator,
): CdkOutputDataCreator {
  return new CdkOutputDataCreator(native);
}
