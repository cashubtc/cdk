/**
 * cashu-ts-compatible adapter over the native Nitro OutputDataCreator.
 *
 * Wraps the native (CDK Rust) blinding results in cashu-ts `OutputData`
 * instances so the module can be injected wherever cashu-ts expects an
 * `OutputDataCreator`, such as Coco's `outputDataCreator` seam. The native
 * crypto is reused verbatim; this layer only converts types across the
 * boundary.
 */
import { Amount, OutputData } from '@cashu/cashu-ts';
import type {
  AmountLike,
  HasKeysetKeys,
  OutputDataCreator,
  P2PKOptions,
  SerializedBlindedMessage,
  SigFlag,
} from '@cashu/cashu-ts';

import type {
  KeyEntry,
  OutputData as NativeOutputData,
  P2PKOptions as NativeP2PKOptions,
} from './specs/OutputDataCreator.nitro';

/**
 * The subset of the native Nitro OutputDataCreator this adapter drives. Kept
 * structural so a stand-in can be injected in tests.
 */
export interface NativeOutputDataCreator {
  createSingleRandomData(amount: number, keysetId: string): NativeOutputData;
  createRandomData(
    amount: number,
    keysetId: string,
    keys: KeyEntry[],
    customSplit?: number[],
  ): NativeOutputData[];
  createSingleP2PKData(
    p2pk: NativeP2PKOptions,
    amount: number,
    keysetId: string,
  ): NativeOutputData;
  createP2PKData(
    p2pk: NativeP2PKOptions,
    amount: number,
    keysetId: string,
    keys: KeyEntry[],
    customSplit?: number[],
  ): NativeOutputData[];
  createSingleDeterministicData(
    amount: number,
    seed: ArrayBuffer,
    counter: number,
    keysetId: string,
  ): NativeOutputData;
  createDeterministicData(
    amount: number,
    seed: ArrayBuffer,
    counter: number,
    keysetId: string,
    keys: KeyEntry[],
    customSplit?: number[],
  ): NativeOutputData[];
}

const textEncoder = new TextEncoder();

function toArrayBuffer(seed: Uint8Array): ArrayBuffer {
  // Copy so a view with a byte offset or a shared buffer is passed as an exact,
  // standalone ArrayBuffer.
  return seed.slice().buffer;
}

function toNativeAmount(amount: AmountLike): number {
  return Amount.from(amount).toNumber();
}

function toNativeSplit(customSplit?: AmountLike[]): number[] | undefined {
  return customSplit?.map((a) => Amount.from(a).toNumber());
}

function toKeyEntries(keyset: HasKeysetKeys): KeyEntry[] {
  return Object.entries(keyset.keys).map(([amount, pubkey]) => ({
    amount: Number(amount),
    pubkey,
  }));
}

function toNativeSigFlag(sigFlag: SigFlag): string {
  return sigFlag === 'SIG_ALL' ? 'SigAll' : 'SigInputs';
}

function toNativeP2PK(p2pk: P2PKOptions): NativeP2PKOptions {
  // NUT-10/11 (cashu-ts 5.x): `kind` selects P2PK vs HTLC, the receiver pubkey
  // lives in `data`, and `pubkeys` holds only the additional multisig keys.
  if (p2pk.kind === 'HTLC') {
    throw new Error('HTLC spending conditions are not supported by the native OutputDataCreator');
  }
  if (p2pk.blindKeys) {
    throw new Error('blindKeys (P2BK) is not supported by the native OutputDataCreator');
  }
  if (p2pk.additionalTags !== undefined && p2pk.additionalTags.length > 0) {
    throw new Error('additionalTags are not supported by the native OutputDataCreator');
  }
  if (!p2pk.data) {
    throw new Error('P2PK requires a recipient pubkey in the data field');
  }
  return {
    pubkey: p2pk.data,
    additionalPubkeys: p2pk.pubkeys,
    numSigs: p2pk.requiredSignatures,
    locktime: p2pk.locktime,
    refundPubkeys: p2pk.refundKeys,
    numSigsRefund: p2pk.requiredRefundSignatures,
    sigFlag: p2pk.sigFlag === undefined ? undefined : toNativeSigFlag(p2pk.sigFlag),
  };
}

/**
 * Wrap a native blinding result in a cashu-ts OutputData. `B_` is the native
 * compressed-hex point, the blinding factor is the native scalar hex read as a
 * big-endian bigint, and the secret is the UTF-8 bytes of the native secret
 * string, matching cashu-ts's own encoding so `toProof` unblinds correctly.
 */
function wrap(native: NativeOutputData): OutputData {
  const blindedMessage: SerializedBlindedMessage = {
    amount: Amount.from(native.amount),
    B_: native.blindedSecret,
    id: native.keysetId,
  };
  const blindingFactor = BigInt(`0x${native.blindingFactor}`);
  const secret = textEncoder.encode(native.secret);
  return new OutputData(blindedMessage, blindingFactor, secret);
}

/**
 * cashu-ts-compatible OutputDataCreator backed by the native CDK Rust crypto.
 * Construct it with the native HybridObject (or a compatible stand-in) and use
 * it anywhere cashu-ts expects an OutputDataCreator.
 */
export class CdkOutputDataCreator implements OutputDataCreator {
  private readonly native: NativeOutputDataCreator;

  constructor(native: NativeOutputDataCreator) {
    this.native = native;
  }

  createRandomData(
    amount: AmountLike,
    keyset: HasKeysetKeys,
    customSplit?: AmountLike[],
  ): OutputData[] {
    return this.native
      .createRandomData(
        toNativeAmount(amount),
        keyset.id,
        toKeyEntries(keyset),
        toNativeSplit(customSplit),
      )
      .map(wrap);
  }

  createSingleRandomData(amount: AmountLike, keysetId: string): OutputData {
    return wrap(this.native.createSingleRandomData(toNativeAmount(amount), keysetId));
  }

  createP2PKData(
    p2pk: P2PKOptions,
    amount: AmountLike,
    keyset: HasKeysetKeys,
    customSplit?: AmountLike[],
  ): OutputData[] {
    return this.native
      .createP2PKData(
        toNativeP2PK(p2pk),
        toNativeAmount(amount),
        keyset.id,
        toKeyEntries(keyset),
        toNativeSplit(customSplit),
      )
      .map(wrap);
  }

  createSingleP2PKData(p2pk: P2PKOptions, amount: AmountLike, keysetId: string): OutputData {
    return wrap(
      this.native.createSingleP2PKData(toNativeP2PK(p2pk), toNativeAmount(amount), keysetId),
    );
  }

  createDeterministicData(
    amount: AmountLike,
    seed: Uint8Array,
    counter: number,
    keyset: HasKeysetKeys,
    customSplit?: AmountLike[],
  ): OutputData[] {
    return this.native
      .createDeterministicData(
        toNativeAmount(amount),
        toArrayBuffer(seed),
        counter,
        keyset.id,
        toKeyEntries(keyset),
        toNativeSplit(customSplit),
      )
      .map(wrap);
  }

  createSingleDeterministicData(
    amount: AmountLike,
    seed: Uint8Array,
    counter: number,
    keysetId: string,
  ): OutputData {
    return wrap(
      this.native.createSingleDeterministicData(
        toNativeAmount(amount),
        toArrayBuffer(seed),
        counter,
        keysetId,
      ),
    );
  }
}
