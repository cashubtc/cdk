/**
 * Pure unit tests for the CdkOutputDataCreator adapter. These exercise only the
 * cashu-ts <-> native boundary conversion, so they need no native library and no
 * cargo build: the native module is replaced by a recording stub that captures
 * the arguments each method receives and returns canned flat results. That lets
 * a test assert both the conversion IN (cashu-ts types -> native shapes) and the
 * wrapping OUT (native results -> cashu-ts OutputData).
 *
 * Runs under `node --experimental-strip-types` because it imports the .ts source
 * directly.
 */
import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import { CdkOutputDataCreator } from '../src/CdkOutputDataCreator.ts';

const KEYSET_ID = '009a1f293253e41e';
const PUBKEY = '02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc';
const PUBKEY_2 = '03b2744dbfdd12fcgc7e89f4a798b2f1006d73gd06g62fbe21b13ff1cf662c6ed';
const PUBKEY_3 = '02c3855ecgee23gdhd8f9a5b8a9c3g2117e84he17h73gcf32c24gg2dg773d7fe';

// A fixed flat native result. `blindingFactor` is 'ff' so the wrapped bigint is
// deterministic (255n); `secret` is a plain string so the UTF-8 encoding is easy
// to assert.
const CANNED = {
  blindedSecret: '02' + 'a'.repeat(64),
  blindingFactor: 'ff',
  secret: 'hello-secret',
};

function canned(amount, keysetId) {
  return { amount, keysetId, ...CANNED };
}

/**
 * Records each native call as [method, args] and returns canned results so the
 * adapter's wrapping can also be checked.
 */
function makeStub() {
  const calls = [];
  return {
    calls,
    last() {
      return calls[calls.length - 1];
    },
    createSingleRandomData(amount, keysetId) {
      calls.push(['createSingleRandomData', { amount, keysetId }]);
      return canned(amount, keysetId);
    },
    createRandomData(amount, keysetId, keys, customSplit) {
      calls.push(['createRandomData', { amount, keysetId, keys, customSplit }]);
      return [canned(amount, keysetId)];
    },
    createSingleP2PKData(p2pk, amount, keysetId) {
      calls.push(['createSingleP2PKData', { p2pk, amount, keysetId }]);
      return canned(amount, keysetId);
    },
    createP2PKData(p2pk, amount, keysetId, keys, customSplit) {
      calls.push(['createP2PKData', { p2pk, amount, keysetId, keys, customSplit }]);
      return [canned(amount, keysetId)];
    },
    createSingleDeterministicData(amount, seed, counter, keysetId) {
      calls.push(['createSingleDeterministicData', { amount, seed, counter, keysetId }]);
      return canned(amount, keysetId);
    },
    createDeterministicData(amount, seed, counter, keysetId, keys, customSplit) {
      calls.push(['createDeterministicData', { amount, seed, counter, keysetId, keys, customSplit }]);
      return [canned(amount, keysetId)];
    },
  };
}

const keyset = () => ({
  id: KEYSET_ID,
  keys: { 1: PUBKEY, 2: PUBKEY_2, 4: PUBKEY_3 },
});

const EXPECTED_KEY_ENTRIES = [
  { amount: 1, pubkey: PUBKEY },
  { amount: 2, pubkey: PUBKEY_2 },
  { amount: 4, pubkey: PUBKEY_3 },
];

describe('argument conversion', () => {
  it('createSingleRandomData passes number amount and keyset id', () => {
    const stub = makeStub();
    new CdkOutputDataCreator(stub).createSingleRandomData(8, KEYSET_ID);
    assert.deepEqual(stub.last(), ['createSingleRandomData', { amount: 8, keysetId: KEYSET_ID }]);
  });

  it('createRandomData maps keyset.keys to sorted key entries and converts the split', () => {
    const stub = makeStub();
    new CdkOutputDataCreator(stub).createRandomData(6, keyset(), [2, 4]);
    const [, args] = stub.last();
    assert.equal(args.amount, 6);
    assert.equal(args.keysetId, KEYSET_ID);
    assert.deepEqual(args.keys, EXPECTED_KEY_ENTRIES);
    assert.deepEqual(args.customSplit, [2, 4]);
  });

  it('omits the split when none is given', () => {
    const stub = makeStub();
    new CdkOutputDataCreator(stub).createRandomData(2, keyset());
    assert.equal(stub.last()[1].customSplit, undefined);
  });

  it('createSingleDeterministicData forwards counter and copies the seed', () => {
    const stub = makeStub();
    const seed = new Uint8Array([1, 2, 3, 4]);
    new CdkOutputDataCreator(stub).createSingleDeterministicData(1, seed, 7, KEYSET_ID);
    const [, args] = stub.last();
    assert.equal(args.counter, 7);
    assert.equal(args.keysetId, KEYSET_ID);
    assert.ok(args.seed instanceof ArrayBuffer);
    assert.deepEqual(new Uint8Array(args.seed), seed);
  });

  it('copies a byte-offset seed view into a standalone ArrayBuffer', () => {
    const stub = makeStub();
    // A view starting at offset 2 into a larger buffer; the adapter must copy
    // exactly the four viewed bytes, not the whole backing buffer.
    const view = new Uint8Array([9, 9, 1, 2, 3, 4]).subarray(2);
    new CdkOutputDataCreator(stub).createDeterministicData(1, view, 0, keyset());
    const { seed } = stub.last()[1];
    assert.ok(seed instanceof ArrayBuffer);
    assert.equal(seed.byteLength, 4);
    assert.deepEqual(new Uint8Array(seed), new Uint8Array([1, 2, 3, 4]));
  });
});

describe('toNativeP2PK field mapping', () => {
  it('maps every P2PK field to its native name', () => {
    const stub = makeStub();
    new CdkOutputDataCreator(stub).createSingleP2PKData(
      {
        kind: 'P2PK',
        data: PUBKEY,
        pubkeys: [PUBKEY_2],
        requiredSignatures: 2,
        locktime: 123,
        refundKeys: [PUBKEY_3],
        requiredRefundSignatures: 1,
        sigFlag: 'SIG_ALL',
      },
      8,
      KEYSET_ID,
    );
    assert.deepEqual(stub.last()[1].p2pk, {
      pubkey: PUBKEY,
      additionalPubkeys: [PUBKEY_2],
      numSigs: 2,
      locktime: 123,
      refundPubkeys: [PUBKEY_3],
      numSigsRefund: 1,
      sigFlag: 'SigAll',
    });
  });

  it('maps SIG_INPUTS and leaves an absent flag undefined', () => {
    const inputs = makeStub();
    new CdkOutputDataCreator(inputs).createSingleP2PKData(
      { kind: 'P2PK', data: PUBKEY, sigFlag: 'SIG_INPUTS' },
      1,
      KEYSET_ID,
    );
    assert.equal(inputs.last()[1].p2pk.sigFlag, 'SigInputs');

    const none = makeStub();
    new CdkOutputDataCreator(none).createSingleP2PKData({ kind: 'P2PK', data: PUBKEY }, 1, KEYSET_ID);
    assert.equal(none.last()[1].p2pk.sigFlag, undefined);
  });
});

describe('toNativeP2PK rejects unsupported conditions', () => {
  const adapter = () => new CdkOutputDataCreator(makeStub());

  it('rejects HTLC', () => {
    assert.throws(
      () => adapter().createSingleP2PKData({ kind: 'HTLC', data: PUBKEY }, 1, KEYSET_ID),
      /HTLC/,
    );
  });

  it('rejects blindKeys (P2BK)', () => {
    assert.throws(
      () => adapter().createSingleP2PKData({ kind: 'P2PK', data: PUBKEY, blindKeys: true }, 1, KEYSET_ID),
      /blindKeys/,
    );
  });

  it('rejects non-empty additionalTags', () => {
    assert.throws(
      () =>
        adapter().createSingleP2PKData(
          { kind: 'P2PK', data: PUBKEY, additionalTags: [['foo', 'bar']] },
          1,
          KEYSET_ID,
        ),
      /additionalTags/,
    );
  });

  it('rejects a missing recipient pubkey', () => {
    assert.throws(
      () => adapter().createSingleP2PKData({ kind: 'P2PK' }, 1, KEYSET_ID),
      /recipient pubkey/,
    );
  });

  it('accepts an empty additionalTags array', () => {
    assert.doesNotThrow(() =>
      adapter().createSingleP2PKData({ kind: 'P2PK', data: PUBKEY, additionalTags: [] }, 1, KEYSET_ID),
    );
  });
});

describe('wrap reconstructs a cashu-ts OutputData', () => {
  it('reads B_, blinding factor (hex bigint), and UTF-8 secret bytes', () => {
    const stub = makeStub();
    const output = new CdkOutputDataCreator(stub).createSingleRandomData(16, KEYSET_ID);

    assert.equal(output.blindedMessage.B_, CANNED.blindedSecret);
    assert.equal(output.blindedMessage.id, KEYSET_ID);
    assert.equal(output.blindedMessage.amount.toNumber(), 16);
    assert.equal(output.blindingFactor, 255n);
    assert.deepEqual(output.secret, new TextEncoder().encode(CANNED.secret));
  });

  it('wraps every element of a list result', () => {
    const stub = makeStub();
    const outputs = new CdkOutputDataCreator(stub).createRandomData(2, keyset());
    assert.equal(outputs.length, 1);
    assert.equal(outputs[0].blindedMessage.B_, CANNED.blindedSecret);
  });
});
