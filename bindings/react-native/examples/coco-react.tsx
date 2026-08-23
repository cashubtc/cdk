/**
 * Using @cashudevkit/react-native with coco's React bindings.
 *
 * Consumer app deps assumed: @cashu/coco-core, @cashu/coco-react,
 * @cashu/coco-expo-sqlite, expo-sqlite, react, @cashu/cashu-ts,
 * react-native-nitro-modules, and @cashudevkit/react-native.
 *
 * CocoCashuProvider takes the same CocoConfig object as initializeCoco, so the
 * native adapter is passed through the `config` prop. Every hook rendered under
 * the provider then uses native crypto.
 */
import React from 'react';
import { CocoCashuProvider, useBalances } from '@cashu/coco-react';
import { SqliteRepositories } from '@cashu/coco-expo-sqlite';
import { openDatabaseSync } from 'expo-sqlite';
import { cashuOutputDataCreator } from '@cashudevkit/react-native/native';

const database = openDatabaseSync('coco.db');
const repo = new SqliteRepositories({ database });

// Return the app's 64-byte BIP-39 seed from secure storage.
async function seedGetter(): Promise<Uint8Array> {
  throw new Error('provide a real seed getter');
}

export function App() {
  return (
    <CocoCashuProvider
      config={{ repo, seedGetter, outputDataCreator: cashuOutputDataCreator }}
      fallback={null}
      errorFallback={null}
    >
      <Balances />
    </CocoCashuProvider>
  );
}

function Balances() {
  const balances = useBalances();
  // Render `balances` however the app needs.
  return <>{JSON.stringify(balances)}</>;
}
