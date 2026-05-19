/**
 * Guards the invariant that non-RN consumers (coco, cashu-ts, and their
 * Node/Bun/Vitest builds) depend on: importing the package root must NOT touch
 * the Nitro runtime. The native HybridObject lives behind the `./native`
 * subpath instead.
 *
 * The check is static on purpose. The root barrel uses extensionless relative
 * imports (resolved by bob/bundlers), so it cannot be executed by raw Node, and
 * the point being protected is a source-level property anyway: `src/index.ts`
 * must not reference `react-native-nitro-modules` or `./native`, and that
 * coupling must stay isolated in `src/native.ts`. The adapter itself is
 * exercised functionally with a pure stub, proving the wrapping works without
 * any native code.
 */
import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { CdkOutputDataCreator } from '../src/CdkOutputDataCreator.ts';

const read = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8');

describe('package root purity', () => {
  const indexSrc = read('../src/index.ts');

  it('root barrel does not import the Nitro runtime', () => {
    assert.ok(
      !indexSrc.includes('react-native-nitro-modules'),
      'src/index.ts must not import react-native-nitro-modules',
    );
    assert.ok(
      !/from\s+['"]\.\/native['"]/.test(indexSrc),
      'src/index.ts must not import ./native',
    );
  });

  it('root barrel exports the pure surface consumers inject', () => {
    for (const name of ['CdkOutputDataCreator', 'createCashuOutputDataCreator']) {
      assert.ok(indexSrc.includes(name), `src/index.ts must export ${name}`);
    }
  });

  it('the Nitro coupling stays isolated in ./native', () => {
    assert.ok(
      read('../src/native.ts').includes('react-native-nitro-modules'),
      'src/native.ts owns the react-native-nitro-modules coupling',
    );
  });
});

describe('adapter works without native code', () => {
  const KEYSET_ID = '009a1f293253e41e';
  const B_ = '02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc';

  // Minimal stand-in for the native OutputDataCreator: fixed flat results so the
  // adapter's type conversion can be checked without native crypto.
  const nativeStub = {
    createSingleRandomData: (amount, keysetId) => ({
      amount,
      keysetId,
      blindedSecret: B_,
      blindingFactor: '01',
      secret: 'deadbeef',
    }),
  };

  it('wraps a native result in a cashu-ts OutputData', () => {
    const adapter = new CdkOutputDataCreator(nativeStub);
    const output = adapter.createSingleRandomData(1, KEYSET_ID);

    assert.equal(output.blindedMessage.B_, B_);
    assert.equal(output.blindedMessage.id, KEYSET_ID);
    assert.equal(output.blindingFactor, 1n);
  });
});
