/**
 * React Native entry point. Importing this module instantiates the native Nitro
 * HybridObject, so it must only be loaded inside a React Native runtime with
 * `react-native-nitro-modules` present. Non-RN consumers (Node/Bun/Vitest
 * typecheck and tests) should import the pure adapter and types from the package
 * root instead, which pulls in no native code.
 */
import { NitroModules } from 'react-native-nitro-modules';
import type { OutputDataCreator as OutputDataCreatorSpec } from './specs/OutputDataCreator.nitro';
import { CdkOutputDataCreator } from './CdkOutputDataCreator';

/** The raw native HybridObject (flat results, no cashu-ts types). */
export const OutputDataCreator =
  NitroModules.createHybridObject<OutputDataCreatorSpec>('OutputDataCreator');

/**
 * A cashu-ts-compatible OutputDataCreator backed by the native module. Inject
 * this into any code expecting a cashu-ts `OutputDataCreator`.
 */
export const cashuOutputDataCreator = new CdkOutputDataCreator(OutputDataCreator);
