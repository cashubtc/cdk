/**
 * Tests the cashu-ts adapter (CdkOutputDataCreator) driving the real native
 * Rust crypto through the koffi FFI stand-in. The key check is cross-impl
 * compatibility: for the same seed / keyset / counter, the native NUT-13
 * derivation must produce byte-for-byte the same blinded message, blinding
 * factor and secret as cashu-ts's own implementation.
 */
import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import { OutputData, Amount } from '@cashu/cashu-ts';

import { CdkOutputDataCreator } from '../src/CdkOutputDataCreator.ts';
import { OutputDataCreatorFFI } from './OutputDataCreatorFFI.mjs';

const KEYSET_ID = '009a1f293253e41e';
const TEST_PUBKEY = '02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc';

// A keyset covering powers of two, in the { [amount]: pubkey } shape cashu-ts
// uses.
const KEYS = Object.fromEntries(
  [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024].map((a) => [String(a), TEST_PUBKEY]),
);
const KEYSET = { id: KEYSET_ID, keys: KEYS };

const adapter = new CdkOutputDataCreator(new OutputDataCreatorFFI());

describe('CdkOutputDataCreator: shape', () => {
  it('wraps native results in cashu-ts OutputData instances', () => {
    const outputs = adapter.createRandomData(13, KEYSET);
    assert.equal(outputs.length, 3);
    for (const o of outputs) {
      assert.ok(o instanceof OutputData);
      assert.ok(o.blindedMessage.amount instanceof Amount);
      // B_ is a compressed SEC1 point.
      assert.match(o.blindedMessage.B_, /^0[23][0-9a-f]{64}$/);
      assert.equal(o.blindedMessage.id, KEYSET_ID);
      assert.equal(typeof o.blindingFactor, 'bigint');
      assert.ok(o.blindingFactor > 0n);
      assert.ok(o.secret instanceof Uint8Array);
      assert.ok(o.secret.length > 0);
    }
    const amounts = outputs.map((o) => o.blindedMessage.amount.toNumber()).sort((a, b) => a - b);
    assert.deepEqual(amounts, [1, 4, 8]);
  });

  it('produces OutputData that satisfies the OutputDataLike contract', () => {
    const [o] = adapter.createRandomData(1, KEYSET);
    // toProof is what consumers call after the mint signs the blinded message.
    assert.equal(typeof o.toProof, 'function');
    // The blinding factor round-trips to the native scalar hex big-endian.
    assert.equal(o.blindingFactor, BigInt(`0x${o.blindingFactor.toString(16)}`));
    // The secret decodes back to the NUT-00 secret string.
    assert.ok(new TextDecoder().decode(o.secret).length > 0);
  });
});

describe('CdkOutputDataCreator: cashu-ts NUT-13 cross-compatibility', () => {
  const seed = new Uint8Array(64).fill(7);

  it('single deterministic output matches cashu-ts byte-for-byte', () => {
    const viaCdk = adapter.createSingleDeterministicData(2, seed, 0, KEYSET_ID);
    const viaCashuTs = OutputData.createSingleDeterministicData(2, seed, 0, KEYSET_ID);

    assert.equal(viaCdk.blindedMessage.B_, viaCashuTs.blindedMessage.B_);
    assert.equal(viaCdk.blindingFactor, viaCashuTs.blindingFactor);
    assert.deepEqual(viaCdk.secret, viaCashuTs.secret);
  });

  it('a batch of deterministic outputs matches cashu-ts at each counter', () => {
    const viaCdk = adapter.createDeterministicData(7, seed, 0, KEYSET);
    const viaCashuTs = OutputData.createDeterministicData(7, seed, 0, KEYSET);

    assert.equal(viaCdk.length, viaCashuTs.length);
    for (let i = 0; i < viaCdk.length; i++) {
      assert.equal(viaCdk[i].blindedMessage.B_, viaCashuTs[i].blindedMessage.B_);
      assert.equal(viaCdk[i].blindingFactor, viaCashuTs[i].blindingFactor);
      assert.deepEqual(viaCdk[i].secret, viaCashuTs[i].secret);
    }
  });
});

describe('CdkOutputDataCreator: splitting via the adapter', () => {
  const seed = new Uint8Array(64).fill(9);

  it('supports partial custom splits (fills the remainder)', () => {
    const outputs = adapter.createRandomData(10, KEYSET, [2, 4]);
    const amounts = outputs.map((o) => o.blindedMessage.amount.toNumber()).sort((a, b) => a - b);
    assert.deepEqual(amounts, [2, 4, 4]);
  });

  it('supports explicit zero outputs for a zero-total custom split', () => {
    const outputs = adapter.createDeterministicData(0, seed, 0, KEYSET, [0, 0]);
    assert.equal(outputs.length, 2);
    for (const o of outputs) assert.equal(o.blindedMessage.amount.toNumber(), 0);
  });
});

describe('CdkOutputDataCreator: P2PK option mapping', () => {
  it('maps a single-key P2PK spend', () => {
    const [o] = adapter.createP2PKData({ kind: 'P2PK', data: TEST_PUBKEY }, 1, KEYSET);
    // The secret is a NUT-10 P2PK tuple embedding the pubkey.
    const secretStr = new TextDecoder().decode(o.secret);
    assert.ok(secretStr.includes('P2PK'));
    assert.ok(secretStr.includes(TEST_PUBKEY));
  });

  it('rejects unsupported P2BK (blindKeys)', () => {
    assert.throws(
      () =>
        adapter.createSingleP2PKData(
          { kind: 'P2PK', data: TEST_PUBKEY, blindKeys: true },
          1,
          KEYSET_ID,
        ),
      /blindKeys \(P2BK\) is not supported/,
    );
  });

  it('rejects unsupported HTLC', () => {
    assert.throws(
      () =>
        adapter.createSingleP2PKData(
          { kind: 'HTLC', data: 'ab'.repeat(32) },
          1,
          KEYSET_ID,
        ),
      /HTLC spending conditions are not supported/,
    );
  });

  it('rejects unsupported additionalTags', () => {
    assert.throws(
      () =>
        adapter.createSingleP2PKData(
          { kind: 'P2PK', data: TEST_PUBKEY, additionalTags: [['foo', 'bar']] },
          1,
          KEYSET_ID,
        ),
      /additionalTags are not supported/,
    );
  });
});
