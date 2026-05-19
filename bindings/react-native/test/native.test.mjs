/**
 * Node.js tests for the cdk-nitro C API.
 *
 * Loads the compiled shared library via koffi and exercises every
 * exported function, mirroring the Rust unit-test coverage.
 */
import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import koffi from 'koffi';
import path from 'node:path';
import { execSync } from 'node:child_process';
import { existsSync } from 'node:fs';

// ---------------------------------------------------------------------------
// Build & load the shared library
// ---------------------------------------------------------------------------

const repoRoot = path.resolve(import.meta.dirname, '..', '..', '..');

function findLib() {
  const base = path.join(repoRoot, 'target', 'debug');
  for (const name of ['libcdk_nitro.dylib', 'libcdk_nitro.so', 'cdk_nitro.dll']) {
    const p = path.join(base, name);
    if (existsSync(p)) return p;
  }
  return null;
}

// Build once before all tests
let libPath = findLib();
if (!libPath) {
  execSync('cargo build -p cdk-nitro', { cwd: repoRoot, stdio: 'inherit' });
  libPath = findLib();
}
assert.ok(libPath, 'shared library must exist after build');

const lib = koffi.load(libPath);

// ---------------------------------------------------------------------------
// FFI declarations
// ---------------------------------------------------------------------------

const CdkBlindResult = koffi.struct('CdkBlindResult', {
  blinded_secret: 'str',
  blinding_factor: 'str',
  secret: 'str',
});

const CdkBlindResultPtr = koffi.pointer(CdkBlindResult);

const cdk_blind_result_free = lib.func('void cdk_blind_result_free(CdkBlindResult *result)');

const cdk_create_random_blinded_message = lib.func(
  'CdkBlindResult *cdk_create_random_blinded_message(uint64_t amount, const char *keyset_id)',
);

const cdk_create_p2pk_blinded_message = lib.func(
  'CdkBlindResult *cdk_create_p2pk_blinded_message(' +
    'uint64_t amount, const char *keyset_id, const char *pubkey_hex, ' +
    'const char **additional_pubkeys, uint32_t additional_pubkeys_len, ' +
    'uint64_t num_sigs, uint64_t locktime, ' +
    'const char **refund_pubkeys, uint32_t refund_pubkeys_len, ' +
    'uint64_t num_sigs_refund, const char *sig_flag)',
);

const cdk_create_deterministic_blinded_message = lib.func(
  'CdkBlindResult *cdk_create_deterministic_blinded_message(' +
    'uint64_t amount, const char *keyset_id, ' +
    'const uint8_t *seed, uint32_t seed_len, uint32_t counter)',
);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const KEYSET_ID = '00456a94ab4e1c46';

function isHex(s) {
  return typeof s === 'string' && s.length > 0 && /^[0-9a-f]+$/i.test(s);
}

/** Decode an opaque pointer into a JS object, returns null if ptr is null. */
function decode(ptr) {
  if (ptr === null) return null;
  return koffi.decode(ptr, CdkBlindResult);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('cdk_create_random_blinded_message', () => {
  it('returns non-null with valid hex fields', () => {
    const ptr = cdk_create_random_blinded_message(1, KEYSET_ID);
    assert.ok(ptr, 'result pointer should not be null');
    const res = decode(ptr);
    assert.ok(isHex(res.blinded_secret), `blinded_secret should be hex, got: ${res.blinded_secret}`);
    assert.ok(isHex(res.blinding_factor), `blinding_factor should be hex, got: ${res.blinding_factor}`);
    assert.ok(res.secret, 'secret should be non-empty');
    cdk_blind_result_free(ptr);
  });

  it('produces unique outputs on each call', () => {
    const p1 = cdk_create_random_blinded_message(1, KEYSET_ID);
    const p2 = cdk_create_random_blinded_message(1, KEYSET_ID);
    assert.ok(p1 && p2);
    const r1 = decode(p1);
    const r2 = decode(p2);
    assert.notEqual(r1.secret, r2.secret, 'secrets must differ');
    cdk_blind_result_free(p1);
    cdk_blind_result_free(p2);
  });
});

describe('cdk_create_p2pk_blinded_message', () => {
  // A valid secp256k1 compressed public key (33 bytes hex)
  const PUBKEY = '02' + 'a'.repeat(64);

  it('returns non-null for a valid pubkey', () => {
    const ptr = cdk_create_p2pk_blinded_message(
      1, KEYSET_ID, PUBKEY, null, 0, 1, 0, null, 0, 0, 'SigInputs',
    );
    assert.ok(ptr, 'result should not be null');
    const res = decode(ptr);
    assert.ok(res.secret.includes('P2PK'), 'secret should contain P2PK spending conditions');
    cdk_blind_result_free(ptr);
  });

  it('returns null for an invalid pubkey', () => {
    const ptr = cdk_create_p2pk_blinded_message(
      1, KEYSET_ID, 'not-a-key', null, 0, 1, 0, null, 0, 0, 'SigInputs',
    );
    assert.equal(ptr, null, 'invalid pubkey should return null');
  });
});

describe('cdk_create_deterministic_blinded_message', () => {
  const seed = Buffer.alloc(64, 42);

  it('returns non-null with a 64-byte seed', () => {
    const ptr = cdk_create_deterministic_blinded_message(1, KEYSET_ID, seed, 64, 0);
    assert.ok(ptr, 'result should not be null');
    const res = decode(ptr);
    assert.ok(isHex(res.blinded_secret));
    cdk_blind_result_free(ptr);
  });

  it('same inputs produce same outputs', () => {
    const p1 = cdk_create_deterministic_blinded_message(1, KEYSET_ID, seed, 64, 0);
    const p2 = cdk_create_deterministic_blinded_message(1, KEYSET_ID, seed, 64, 0);
    assert.ok(p1 && p2);
    const r1 = decode(p1);
    const r2 = decode(p2);
    assert.equal(r1.secret, r2.secret, 'same inputs must produce same secret');
    assert.equal(r1.blinded_secret, r2.blinded_secret);
    cdk_blind_result_free(p1);
    cdk_blind_result_free(p2);
  });

  it('different counters produce different outputs', () => {
    const p1 = cdk_create_deterministic_blinded_message(1, KEYSET_ID, seed, 64, 0);
    const p2 = cdk_create_deterministic_blinded_message(1, KEYSET_ID, seed, 64, 1);
    assert.ok(p1 && p2);
    const r1 = decode(p1);
    const r2 = decode(p2);
    assert.notEqual(r1.secret, r2.secret, 'different counters must produce different secrets');
    cdk_blind_result_free(p1);
    cdk_blind_result_free(p2);
  });

  it('different seeds produce different outputs', () => {
    const seedA = Buffer.alloc(64, 1);
    const seedB = Buffer.alloc(64, 2);
    const p1 = cdk_create_deterministic_blinded_message(1, KEYSET_ID, seedA, 64, 0);
    const p2 = cdk_create_deterministic_blinded_message(1, KEYSET_ID, seedB, 64, 0);
    assert.ok(p1 && p2);
    const r1 = decode(p1);
    const r2 = decode(p2);
    assert.notEqual(r1.secret, r2.secret, 'different seeds must produce different secrets');
    cdk_blind_result_free(p1);
    cdk_blind_result_free(p2);
  });

  it('returns null for wrong seed length', () => {
    const shortSeed = Buffer.alloc(32, 0);
    const ptr = cdk_create_deterministic_blinded_message(1, KEYSET_ID, shortSeed, 32, 0);
    assert.equal(ptr, null, 'seed != 64 bytes should return null');
  });
});

describe('cdk_blind_result_free', () => {
  it('does not crash on null', () => {
    cdk_blind_result_free(null);
  });
});
