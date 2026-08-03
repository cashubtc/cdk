/**
 * Using @cashudevkit/react-native with coco (headless).
 *
 * Consumer app deps assumed: @cashu/coco-core, @cashu/coco-expo-sqlite,
 * expo-sqlite, @cashu/cashu-ts, react-native-nitro-modules, and
 * @cashudevkit/react-native.
 *
 * coco exposes `outputDataCreator` as a first-class CocoConfig field, so the
 * native adapter is injected once at initialization and reused across every
 * output-producing flow (mint, receive, send, restore).
 */
import { initializeCoco } from '@cashu/coco-core';
import { SqliteRepositories } from '@cashu/coco-expo-sqlite';
import { openDatabaseAsync } from 'expo-sqlite';
import { cashuOutputDataCreator } from '@cashudevkit/react-native/native';

/**
 * @param seedGetter returns the app's 64-byte BIP-39 seed; coco never persists
 *   it. Back it with secure storage (e.g. expo-secure-store).
 */
export async function createCocoWallet(
  seedGetter: () => Promise<Uint8Array>,
  mintUrl: string,
) {
  const database = await openDatabaseAsync('coco.db');
  const repo = new SqliteRepositories({ database });

  // initializeCoco calls repo.init() for you.
  const coco = await initializeCoco({
    repo,
    seedGetter,
    // Native DHKE blinding / NUT-13 derivation, used by all wallet operations.
    outputDataCreator: cashuOutputDataCreator,
  });

  await coco.mint.addMint(mintUrl, { trusted: true });
  return coco;
}

export async function receiveToken(
  coco: Awaited<ReturnType<typeof createCocoWallet>>,
  token: string,
) {
  await coco.wallet.receive(token);
  return coco.wallet.balances.byMint();
}
