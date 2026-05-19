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
    'double num_sigs, double locktime, ' +
    'const char **refund_pubkeys, uint32_t refund_pubkeys_len, ' +
    'double num_sigs_refund, const char *sig_flag)',
);

const cdk_create_deterministic_blinded_message = lib.func(
  'CdkBlindResult *cdk_create_deterministic_blinded_message(' +
    'uint64_t amount, const char *keyset_id, ' +
    'const uint8_t *seed, uint32_t seed_len, double counter)',
);

const CdkBlindResultList = koffi.struct('CdkBlindResultList', {
  items: 'CdkBlindResult *',
  amounts: 'uint64_t *',
  len: 'size_t',
  error: 'str',
});

const cdk_blind_result_list_free = lib.func(
  'void cdk_blind_result_list_free(CdkBlindResultList *list)',
);

const cdk_create_random_outputs = lib.func(
  'CdkBlindResultList *cdk_create_random_outputs(' +
    'double amount, const char *keyset_id, ' +
    'const double *denoms, size_t denoms_len, ' +
    'const double *custom_split, size_t custom_split_len)',
);

const cdk_create_p2pk_outputs = lib.func(
  'CdkBlindResultList *cdk_create_p2pk_outputs(' +
    'double amount, const char *keyset_id, ' +
    'const double *denoms, size_t denoms_len, ' +
    'const double *custom_split, size_t custom_split_len, ' +
    'const char *pubkey_hex, ' +
    'const char **additional_pubkeys, uint32_t additional_pubkeys_len, ' +
    'double num_sigs, double locktime, ' +
    'const char **refund_pubkeys, uint32_t refund_pubkeys_len, ' +
    'double num_sigs_refund, const char *sig_flag)',
);

const cdk_create_deterministic_outputs = lib.func(
  'CdkBlindResultList *cdk_create_deterministic_outputs(' +
    'double amount, const char *keyset_id, ' +
    'const uint8_t *seed, uint32_t seed_len, double counter, ' +
    'const double *denoms, size_t denoms_len, ' +
    'const double *custom_split, size_t custom_split_len)',
);

// ---------------------------------------------------------------------------
// Marshalling helpers
// ---------------------------------------------------------------------------

/** Keyset key entries reduced to a plain denomination array (or null). */
function denomsOf(keys) {
  return keys && keys.length > 0 ? keys.map((k) => k.amount) : null;
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

/**
 * Decode a CdkBlindResultList pointer into an array of OutputData, rethrowing
 * the Rust error message when present, then free the list.
 */
function decodeList(listPtr, keysetId) {
  if (listPtr === null) throw new Error('cdk-nitro returned no result');
  const list = koffi.decode(listPtr, CdkBlindResultList);
  try {
    if (list.error) throw new Error(list.error);
    if (list.len === 0) return [];
    const items = koffi.decode(list.items, CdkBlindResult, list.len);
    const amounts = koffi.decode(list.amounts, 'uint64_t', list.len);
    return items.map((raw, i) => ({
      amount: Number(amounts[i]),
      keysetId,
      blindedSecret: raw.blinded_secret,
      blindingFactor: raw.blinding_factor,
      secret: raw.secret,
    }));
  } finally {
    cdk_blind_result_list_free(listPtr);
  }
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
    const listPtr = cdk_create_random_outputs(
      amount,
      keysetId,
      denomsOf(keys),
      keys?.length ?? 0,
      customSplit ?? null,
      customSplit?.length ?? 0,
    );
    return decodeList(listPtr, keysetId);
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
      p2pk.numSigsRefund ?? 0,
      p2pk.sigFlag ?? 'SigInputs',
    );
    const result = decodeResult(ptr, amount, keysetId);
    if (!result) throw new Error('Failed to create P2PK blinded message');
    return result;
  }

  createP2PKData(p2pk, amount, keysetId, keys, customSplit) {
    const listPtr = cdk_create_p2pk_outputs(
      amount,
      keysetId,
      denomsOf(keys),
      keys?.length ?? 0,
      customSplit ?? null,
      customSplit?.length ?? 0,
      p2pk.pubkey,
      p2pk.additionalPubkeys ?? null,
      p2pk.additionalPubkeys?.length ?? 0,
      p2pk.numSigs ?? 1,
      p2pk.locktime ?? 0,
      p2pk.refundPubkeys ?? null,
      p2pk.refundPubkeys?.length ?? 0,
      p2pk.numSigsRefund ?? 0,
      p2pk.sigFlag ?? 'SigInputs',
    );
    return decodeList(listPtr, keysetId);
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
    const seedBuf = seed instanceof ArrayBuffer ? Buffer.from(seed) : seed;
    const listPtr = cdk_create_deterministic_outputs(
      amount,
      keysetId,
      seedBuf,
      seedBuf.length,
      counter,
      denomsOf(keys),
      keys?.length ?? 0,
      customSplit ?? null,
      customSplit?.length ?? 0,
    );
    return decodeList(listPtr, keysetId);
  }
}
