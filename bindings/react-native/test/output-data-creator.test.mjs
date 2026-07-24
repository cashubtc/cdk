/**
 * TypeScript-level API tests for OutputDataCreator.
 *
 * Tests the same interface that React Native consumers use
 * (HybridOutputDataCreator), backed by a Node.js FFI adapter.
 */
import { describe, it, before } from 'node:test';
import assert from 'node:assert/strict';
import { OutputDataCreatorFFI } from './OutputDataCreatorFFI.mjs';

const KEYSET_ID = '009a1f293253e41e';
const TEST_PUBKEY = '02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc';

// Mint keyset keys (power-of-2 denominations)
const KEYS = [1, 2, 4, 8, 16, 32, 64].map((amount) => ({
  amount,
  pubkey: TEST_PUBKEY,
}));

function isHex(s) {
  return typeof s === 'string' && s.length > 0 && /^[0-9a-f]+$/i.test(s);
}

function isCompressedPubkey(s) {
  return typeof s === 'string' && s.length === 66 && (s.startsWith('02') || s.startsWith('03'));
}

/** @type {OutputDataCreatorFFI} */
let creator;

before(() => {
  creator = new OutputDataCreatorFFI();
});

// ---------------------------------------------------------------------------
// Random outputs
// ---------------------------------------------------------------------------

describe('createSingleRandomData', () => {
  it('returns valid OutputData with correct fields', () => {
    const output = creator.createSingleRandomData(64, KEYSET_ID);

    assert.equal(output.amount, 64);
    assert.equal(output.keysetId, KEYSET_ID);
    assert.ok(isCompressedPubkey(output.blindedSecret), `expected compressed pubkey, got: ${output.blindedSecret}`);
    assert.ok(isHex(output.blindingFactor), `expected hex blinding factor, got: ${output.blindingFactor}`);
    assert.equal(output.blindingFactor.length, 64, 'blinding factor should be 32 bytes (64 hex chars)');
    assert.ok(output.secret.length > 0, 'secret should be non-empty');
  });

  it('produces unique outputs on each call', () => {
    const a = creator.createSingleRandomData(1, KEYSET_ID);
    const b = creator.createSingleRandomData(1, KEYSET_ID);

    assert.notEqual(a.secret, b.secret, 'secrets must differ');
    assert.notEqual(a.blindedSecret, b.blindedSecret, 'blinded secrets must differ');
    assert.notEqual(a.blindingFactor, b.blindingFactor, 'blinding factors must differ');
  });
});

describe('createRandomData', () => {
  it('splits amount into correct denominations using keys', () => {
    const outputs = creator.createRandomData(13, KEYSET_ID, KEYS);

    // 13 = 8 + 4 + 1
    assert.equal(outputs.length, 3);
    const amounts = outputs.map((o) => o.amount).sort((a, b) => a - b);
    assert.deepEqual(amounts, [1, 4, 8]);
  });

  it('uses custom split when provided', () => {
    const outputs = creator.createRandomData(10, KEYSET_ID, KEYS, [2, 2, 2, 2, 2]);

    assert.equal(outputs.length, 5);
    for (const o of outputs) {
      assert.equal(o.amount, 2);
      assert.equal(o.keysetId, KEYSET_ID);
    }
  });

  it('all outputs have valid hex fields', () => {
    const outputs = creator.createRandomData(7, KEYSET_ID, KEYS);

    for (const o of outputs) {
      assert.ok(isCompressedPubkey(o.blindedSecret));
      assert.ok(isHex(o.blindingFactor));
      assert.ok(o.secret.length > 0);
    }
  });

  it('all outputs are unique', () => {
    const outputs = creator.createRandomData(15, KEYSET_ID, KEYS);
    const secrets = outputs.map((o) => o.secret);
    const unique = new Set(secrets);
    assert.equal(unique.size, secrets.length, 'all secrets must be unique');
  });
});

// ---------------------------------------------------------------------------
// Split validation (mirrors HybridOutputDataCreator::splitAmount)
// ---------------------------------------------------------------------------

describe('split validation', () => {
  // Keyset missing the low denominations needed to represent odd amounts.
  const COARSE_KEYS = [8, 16, 32].map((amount) => ({ amount, pubkey: TEST_PUBKEY }));

  it('throws when keys cannot represent the amount exactly', () => {
    assert.throws(
      () => creator.createRandomData(5, KEYSET_ID, COARSE_KEYS),
      /Cannot split amount with available denominations/,
    );
  });

  it('throws when a custom split sum does not equal the amount', () => {
    assert.throws(
      () => creator.createRandomData(10, KEYSET_ID, KEYS, [2, 4]),
      /Custom split total does not equal requested amount/,
    );
  });

  it('throws when a custom split contains a zero denomination', () => {
    assert.throws(
      () => creator.createRandomData(6, KEYSET_ID, KEYS, [2, 4, 0]),
      /Custom split contains invalid denomination/,
    );
  });
});

// ---------------------------------------------------------------------------
// P2PK outputs
// ---------------------------------------------------------------------------

describe('createSingleP2PKData', () => {
  it('returns valid OutputData with P2PK spending conditions', () => {
    const p2pk = { pubkey: TEST_PUBKEY };
    const output = creator.createSingleP2PKData(p2pk, 64, KEYSET_ID);

    assert.equal(output.amount, 64);
    assert.equal(output.keysetId, KEYSET_ID);
    assert.ok(isCompressedPubkey(output.blindedSecret));
    assert.ok(output.secret.includes('P2PK'), 'secret should contain P2PK kind');
    assert.ok(output.secret.includes(TEST_PUBKEY), 'secret should embed recipient pubkey');
  });

  it('throws for invalid pubkey', () => {
    const p2pk = { pubkey: 'not-a-valid-pubkey' };
    assert.throws(
      () => creator.createSingleP2PKData(p2pk, 1, KEYSET_ID),
      /Failed to create P2PK/,
    );
  });

  it('supports locktime', () => {
    // Far-future locktime; the validated constructor rejects past ones.
    const p2pk = { pubkey: TEST_PUBKEY, locktime: 4102444800 };
    const output = creator.createSingleP2PKData(p2pk, 1, KEYSET_ID);

    assert.ok(output.secret.includes('4102444800'), 'locktime should appear in secret');
  });

  it('supports multisig with additional pubkeys', () => {
    const p2pk = {
      pubkey: TEST_PUBKEY,
      additionalPubkeys: [TEST_PUBKEY],
      numSigs: 2,
    };
    const output = creator.createSingleP2PKData(p2pk, 1, KEYSET_ID);

    assert.ok(isCompressedPubkey(output.blindedSecret));
    assert.ok(output.secret.includes('P2PK'));
  });

  it('supports refund pubkeys', () => {
    const p2pk = {
      pubkey: TEST_PUBKEY,
      refundPubkeys: [TEST_PUBKEY],
      locktime: 4102444800,
    };
    const output = creator.createSingleP2PKData(p2pk, 1, KEYSET_ID);

    assert.ok(output.secret.includes('P2PK'));
    assert.ok(output.secret.includes('4102444800'));
  });
});

describe('createP2PKData', () => {
  it('splits amount into outputs with P2PK conditions', () => {
    const p2pk = { pubkey: TEST_PUBKEY };
    const outputs = creator.createP2PKData(p2pk, 13, KEYSET_ID, KEYS);

    // 13 = 8 + 4 + 1
    assert.equal(outputs.length, 3);
    const amounts = outputs.map((o) => o.amount).sort((a, b) => a - b);
    assert.deepEqual(amounts, [1, 4, 8]);

    for (const o of outputs) {
      assert.ok(o.secret.includes('P2PK'));
    }
  });

  it('uses custom split', () => {
    const p2pk = { pubkey: TEST_PUBKEY };
    const outputs = creator.createP2PKData(p2pk, 6, KEYSET_ID, KEYS, [2, 4]);

    assert.equal(outputs.length, 2);
    assert.deepEqual(
      outputs.map((o) => o.amount).sort((a, b) => a - b),
      [2, 4],
    );
  });
});

// ---------------------------------------------------------------------------
// Deterministic outputs (NUT-13)
// ---------------------------------------------------------------------------

describe('createSingleDeterministicData', () => {
  const seed = new Uint8Array(64).fill(42).buffer;

  it('returns valid OutputData', () => {
    const output = creator.createSingleDeterministicData(1, seed, 0, KEYSET_ID);

    assert.equal(output.amount, 1);
    assert.equal(output.keysetId, KEYSET_ID);
    assert.ok(isCompressedPubkey(output.blindedSecret));
    assert.ok(isHex(output.blindingFactor));
    assert.ok(output.secret.length > 0);
  });

  it('same inputs produce same outputs (determinism)', () => {
    const a = creator.createSingleDeterministicData(1, seed, 0, KEYSET_ID);
    const b = creator.createSingleDeterministicData(1, seed, 0, KEYSET_ID);

    assert.equal(a.secret, b.secret);
    assert.equal(a.blindedSecret, b.blindedSecret);
    assert.equal(a.blindingFactor, b.blindingFactor);
  });

  it('different counters produce different outputs', () => {
    const a = creator.createSingleDeterministicData(1, seed, 0, KEYSET_ID);
    const b = creator.createSingleDeterministicData(1, seed, 1, KEYSET_ID);

    assert.notEqual(a.secret, b.secret, 'different counters must produce different secrets');
    assert.notEqual(a.blindedSecret, b.blindedSecret);
  });

  it('different seeds produce different outputs', () => {
    const seedA = new Uint8Array(64).fill(1).buffer;
    const seedB = new Uint8Array(64).fill(2).buffer;

    const a = creator.createSingleDeterministicData(1, seedA, 0, KEYSET_ID);
    const b = creator.createSingleDeterministicData(1, seedB, 0, KEYSET_ID);

    assert.notEqual(a.secret, b.secret, 'different seeds must produce different secrets');
  });

  it('throws for wrong seed length', () => {
    const shortSeed = new Uint8Array(32).buffer;
    assert.throws(
      () => creator.createSingleDeterministicData(1, shortSeed, 0, KEYSET_ID),
      /Failed to create deterministic/,
    );
  });
});

describe('createDeterministicData', () => {
  const seed = new Uint8Array(64).fill(42).buffer;

  it('splits amount into deterministic outputs', () => {
    const outputs = creator.createDeterministicData(13, seed, 0, KEYSET_ID, KEYS);

    // 13 = 8 + 4 + 1
    assert.equal(outputs.length, 3);
    const amounts = outputs.map((o) => o.amount).sort((a, b) => a - b);
    assert.deepEqual(amounts, [1, 4, 8]);
  });

  it('outputs are deterministic', () => {
    const a = creator.createDeterministicData(7, seed, 0, KEYSET_ID, KEYS);
    const b = creator.createDeterministicData(7, seed, 0, KEYSET_ID, KEYS);

    assert.equal(a.length, b.length);
    for (let i = 0; i < a.length; i++) {
      assert.equal(a[i].secret, b[i].secret);
      assert.equal(a[i].blindedSecret, b[i].blindedSecret);
    }
  });

  it('each output uses incrementing counter', () => {
    const outputs = creator.createDeterministicData(3, seed, 10, KEYSET_ID, KEYS);

    // 3 = 2 + 1 → 2 outputs, counters 10 and 11
    assert.equal(outputs.length, 2);

    // Verify they match individual calls with those counters
    const single10 = creator.createSingleDeterministicData(2, seed, 10, KEYSET_ID);
    const single11 = creator.createSingleDeterministicData(1, seed, 11, KEYSET_ID);

    assert.equal(outputs[0].secret, single10.secret);
    assert.equal(outputs[1].secret, single11.secret);
  });

  it('uses custom split', () => {
    const outputs = creator.createDeterministicData(10, seed, 0, KEYSET_ID, KEYS, [4, 4, 2]);

    assert.equal(outputs.length, 3);
    assert.deepEqual(
      outputs.map((o) => o.amount).sort((a, b) => a - b),
      [2, 4, 4],
    );
  });
});
