import { NitroModules } from 'react-native-nitro-modules';
import type { HybridOutputDataCreator } from './specs/OutputDataCreator.nitro';

export type { OutputData, KeyEntry, P2PKOptions, HybridOutputDataCreator } from './specs/OutputDataCreator.nitro';

export const OutputDataCreator =
  NitroModules.createHybridObject<HybridOutputDataCreator>('OutputDataCreator');
