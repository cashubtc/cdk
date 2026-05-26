/**
 * Node.js adapter that wraps the cdk-nitro C FFI to expose the same API
 * as the HybridOutputDataCreator Nitro interface.
 *
 * This allows testing the TypeScript-level API surface without requiring
 * a React Native runtime.
 */
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

let libPath = findLib();
if (!libPath) {
  execSync('cargo build -p cdk-nitro', { cwd: repoRoot, stdio: 'inherit' });
  libPath = findLib();
}
if (!libPath) throw new Error('shared library not found after build');

const lib = koffi.load(libPath);

// ---------------------------------------------------------------------------
// FFI declarations
// ---------------------------------------------------------------------------

const CdkBlindResult = koffi.struct('CdkBlindResult', {
  blinded_secret: 'str',
  blinding_factor: 'str',
  secret: 'str',
});

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
    'const char *sig_flag)',
);

const cdk_create_deterministic_blinded_message = lib.func(
  'CdkBlindResult *cdk_create_deterministic_blinded_message(' +
    'uint64_t amount, const char *keyset_id, ' +
    'const uint8_t *seed, uint32_t seed_len, uint32_t counter)',
);

// ---------------------------------------------------------------------------
// Amount splitting (Cashu power-of-2 decomposition)
// ---------------------------------------------------------------------------

/**
 * Split an amount into powers of 2.
 * e.g. 13 → [1, 4, 8]
 */
function splitPow2(amount) {
  const splits = [];
  let remaining = amount;
  let power = 1;
  while (remaining > 0) {
    if (remaining & 1) splits.push(power);
    remaining >>= 1;
    power <<= 1;
  }
  return splits;
}

/**
 * Split amount using available key denominations.
 * Greedily picks the largest denomination that fits.
 */
function splitByKeys(amount, keys) {
  const denoms = keys.map((k) => k.amount).sort((a, b) => b - a);
  const splits = [];
  let remaining = amount;
  for (const d of denoms) {
    while (remaining >= d) {
      splits.push(d);
      remaining -= d;
    }
  }
  if (remaining > 0) {
    // Fallback: add remainder as-is (shouldn't happen with power-of-2 keys)
    splits.push(remaining);
  }
  return splits;
}

function computeSplit(amount, keys, customSplit) {
  if (customSplit && customSplit.length > 0) return customSplit;
  if (keys && keys.length > 0) return splitByKeys(amount, keys);
  return splitPow2(amount);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Decode a CdkBlindResult pointer into an OutputData object. */
function decodeResult(ptr, amount, keysetId) {
  if (ptr === null) return null;
  const raw = koffi.decode(ptr, CdkBlindResult);
  const result = {
    amount,
    keysetId,
    blindedSecret: raw.blinded_secret,
    blindingFactor: raw.blinding_factor,
    secret: raw.secret,
  };
  cdk_blind_result_free(ptr);
  return result;
}

// ---------------------------------------------------------------------------
// OutputDataCreator implementation
// ---------------------------------------------------------------------------

export class OutputDataCreatorFFI {
  // --- Random outputs ---

  createSingleRandomData(amount, keysetId) {
    const ptr = cdk_create_random_blinded_message(amount, keysetId);
    const result = decodeResult(ptr, amount, keysetId);
    if (!result) throw new Error('Failed to create random blinded message');
    return result;
  }

  createRandomData(amount, keysetId, keys, customSplit) {
    const splits = computeSplit(amount, keys, customSplit);
    return splits.map((a) => this.createSingleRandomData(a, keysetId));
  }

  // --- P2PK outputs ---

  createSingleP2PKData(p2pk, amount, keysetId) {
    const ptr = cdk_create_p2pk_blinded_message(
      amount,
      keysetId,
      p2pk.pubkey,
      p2pk.additionalPubkeys ?? null,
      p2pk.additionalPubkeys?.length ?? 0,
      p2pk.numSigs ?? 1,
      p2pk.locktime ?? 0,
      p2pk.refundPubkeys ?? null,
      p2pk.refundPubkeys?.length ?? 0,
      p2pk.sigFlag ?? 'SigInputs',
    );
    const result = decodeResult(ptr, amount, keysetId);
    if (!result) throw new Error('Failed to create P2PK blinded message');
    return result;
  }

  createP2PKData(p2pk, amount, keysetId, keys, customSplit) {
    const splits = computeSplit(amount, keys, customSplit);
    return splits.map((a) => this.createSingleP2PKData(p2pk, a, keysetId));
  }

  // --- Deterministic outputs ---

  createSingleDeterministicData(amount, seed, counter, keysetId) {
    const seedBuf = seed instanceof ArrayBuffer ? Buffer.from(seed) : seed;
    const ptr = cdk_create_deterministic_blinded_message(
      amount,
      keysetId,
      seedBuf,
      seedBuf.length,
      counter,
    );
    const result = decodeResult(ptr, amount, keysetId);
    if (!result) throw new Error('Failed to create deterministic blinded message');
    return result;
  }

  createDeterministicData(amount, seed, counter, keysetId, keys, customSplit) {
    const splits = computeSplit(amount, keys, customSplit);
    return splits.map((a, i) =>
      this.createSingleDeterministicData(a, seed, counter + i, keysetId),
    );
  }
}
