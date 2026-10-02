import {
  Amount,
  OutputData,
  splitAmount,
  type AmountLike,
  type HasKeysetKeys,
  type OutputDataCreator,
  type OutputDataLike,
  type P2PKOptions,
} from '@cashu/cashu-ts';

import { getCashuCrypto } from './index';
import type { BlindedOutput, CashuCrypto, P2pkOptions } from './generated/CashuCrypto.nitro';
import { withNativeErrors } from './generated/CashuCryptoErrors';

const encoder = new TextEncoder();

/**
 * A cashu-ts {@link OutputDataCreator} that builds blinded outputs in Rust.
 *
 * Only the construction of outputs moves native. `toProof` stays on the
 * canonical cashu-ts {@link OutputData}, so the amount binding, DLEQ and BLS
 * checks that protect a wallet against a malicious mint are unchanged.
 *
 * Lives behind the `/creator` subpath because it is the one part of the package
 * that needs cashu-ts at runtime; the root entrypoint stays importable without
 * it.
 *
 * @example
 *
 *     import { NativeOutputDataCreator } from '@cashu/cashu-native/creator';
 *
 *     const wallet = new Wallet(mint, {
 *       outputDataCreator: new NativeOutputDataCreator(),
 *     });
 */
export class NativeOutputDataCreator implements OutputDataCreator {
  private readonly crypto: CashuCrypto;
  private readonly fallback: P2PKFallback;

  /**
   * @param fallback Handles the shapes the native module does not express:
   *   HTLC locks, NUT-28 blinded keys and caller-supplied extra tags. Defaults
   *   to cashu-ts's own implementation.
   */
  constructor(fallback: P2PKFallback = stockP2PKFallback) {
    this.crypto = getCashuCrypto();
    this.fallback = fallback;
  }

  createRandomData(
    amount: AmountLike,
    keyset: HasKeysetKeys,
    customSplit?: AmountLike[],
  ): OutputDataLike[] {
    const amounts = splitAmount(amount, keyset.keys, customSplit);
    const native = withNativeErrors(() =>
      this.crypto.createRandomOutputs(amounts.map(toBigInt), keyset.id),
    );
    return native.map(toOutputData);
  }

  createSingleRandomData(amount: AmountLike, keysetId: string): OutputDataLike {
    return toOutputData(
      withNativeErrors(() =>
        this.crypto.createSingleRandomOutput(toBigInt(amount), keysetId),
      ),
    );
  }

  createDeterministicData(
    amount: AmountLike,
    seed: Uint8Array,
    counter: number,
    keyset: HasKeysetKeys,
    customSplit?: AmountLike[],
  ): OutputDataLike[] {
    const amounts = splitAmount(amount, keyset.keys, customSplit);
    const native = withNativeErrors(() =>
      this.crypto.createDeterministicOutputs(
        amounts.map(toBigInt),
        toArrayBuffer(seed),
        counter,
        keyset.id,
      ),
    );
    return native.map(toOutputData);
  }

  createSingleDeterministicData(
    amount: AmountLike,
    seed: Uint8Array,
    counter: number,
    keysetId: string,
  ): OutputDataLike {
    return toOutputData(
      withNativeErrors(() =>
        this.crypto.createSingleDeterministicOutput(
          toBigInt(amount),
          toArrayBuffer(seed),
          counter,
          keysetId,
        ),
      ),
    );
  }

  createP2PKData(
    p2pk: P2PKOptions,
    amount: AmountLike,
    keyset: HasKeysetKeys,
    customSplit?: AmountLike[],
  ): OutputDataLike[] {
    const options = toNativeP2PK(p2pk);
    if (options === undefined) {
      return this.fallback.createP2PKData(p2pk, amount, keyset, customSplit);
    }
    const amounts = splitAmount(amount, keyset.keys, customSplit);
    const native = withNativeErrors(() =>
      this.crypto.createP2pkOutputs(options, amounts.map(toBigInt), keyset.id),
    );
    return native.map(toOutputData);
  }

  createSingleP2PKData(p2pk: P2PKOptions, amount: AmountLike, keysetId: string): OutputDataLike {
    const options = toNativeP2PK(p2pk);
    if (options === undefined) {
      return this.fallback.createSingleP2PKData(p2pk, amount, keysetId);
    }
    return toOutputData(
      withNativeErrors(() =>
        this.crypto.createSingleP2pkOutput(options, toBigInt(amount), keysetId),
      ),
    );
  }

  /**
   * NUT-09 restore outputs for `count` counters from `start`, in one native call.
   *
   * Restore is the heaviest deterministic workload a wallet runs, so it gets a
   * direct entry point rather than one call per counter. `count` is capped
   * native-side, since the whole batch runs on the calling thread.
   */
  createRestoreData(
    seed: Uint8Array,
    keysetId: string,
    start: number,
    count: number,
  ): OutputDataLike[] {
    const native = withNativeErrors(() =>
      this.crypto.createRestoreOutputs(toArrayBuffer(seed), keysetId, start, count),
    );
    return native.map(toOutputData);
  }
}

/**
 * The subset of {@link OutputDataCreator} used when a lock cannot go native.
 */
export type P2PKFallback = Pick<OutputDataCreator, 'createP2PKData' | 'createSingleP2PKData'>;

const stockP2PKFallback: P2PKFallback = {
  createP2PKData: (p2pk, amount, keyset, customSplit) =>
    OutputData.createP2PKData(p2pk, amount, keyset, customSplit),
  createSingleP2PKData: (p2pk, amount, keysetId) =>
    OutputData.createSingleP2PKData(p2pk, amount, keysetId),
};

function toOutputData(output: BlindedOutput): OutputData {
  return new OutputData(
    {
      amount: Amount.from(output.amount),
      B_: output.blindedSecret,
      id: output.keysetId,
    },
    BigInt('0x' + output.blindingFactor),
    encoder.encode(output.secret),
  );
}

function toBigInt(amount: AmountLike): bigint {
  return Amount.from(amount).toBigInt();
}

function toArrayBuffer(bytes: Uint8Array): ArrayBuffer {
  // A Uint8Array can be a window onto a larger buffer, so slice rather than
  // handing the whole backing store to the native side.
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

/**
 * Map a cashu-ts lock onto the native one, or `undefined` when the native
 * module cannot express it and the caller should fall back.
 */
function toNativeP2PK(p2pk: P2PKOptions): P2pkOptions | undefined {
  if (p2pk.kind !== 'P2PK') return undefined;
  if (p2pk.blindKeys === true) return undefined;
  if (p2pk.additionalTags !== undefined && p2pk.additionalTags.length > 0) return undefined;

  return {
    pubkey: p2pk.data,
    additionalPubkeys: p2pk.pubkeys,
    numSigs: p2pk.requiredSignatures === undefined ? undefined : BigInt(p2pk.requiredSignatures),
    locktime: p2pk.locktime === undefined ? undefined : BigInt(p2pk.locktime),
    refundPubkeys: p2pk.refundKeys,
    numSigsRefund:
      p2pk.requiredRefundSignatures === undefined
        ? undefined
        : BigInt(p2pk.requiredRefundSignatures),
    sigFlag: p2pk.sigFlag === 'SIG_ALL' ? 'sigAll' : 'sigInputs',
  };
}
