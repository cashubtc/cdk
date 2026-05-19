import type { HybridObject } from 'react-native-nitro-modules';

/**
 * Represents blinded output data returned from native crypto operations.
 */
export interface OutputData {
  /** Amount in satoshis */
  amount: number;
  /** Keyset ID (hex string) */
  keysetId: string;
  /** Blinded secret (hex-encoded compressed point) */
  blindedSecret: string;
  /** Blinding factor / secret key used for blinding (hex) */
  blindingFactor: string;
  /** The raw secret (hex) */
  secret: string;
}

/**
 * A single key entry: denomination -> public key hex
 */
export interface KeyEntry {
  amount: number;
  pubkey: string;
}

/**
 * P2PK spending condition options
 */
export interface P2PKOptions {
  /** Recipient public key (hex, 33 bytes compressed) */
  pubkey: string;
  /** Additional pubkeys for multisig */
  additionalPubkeys?: string[];
  /** Number of required signatures (default: 1) */
  numSigs?: number;
  /** Unix locktime for refund */
  locktime?: number;
  /** Refund public keys */
  refundPubkeys?: string[];
  /** Number of required refund signatures (default: 1) */
  numSigsRefund?: number;
  /** Signature flag: 'SigInputs' | 'SigAll' */
  sigFlag?: string;
}

/**
 * Native Cashu output data creator using Rust DHKE cryptography.
 *
 * Implements blinded message construction for the Cashu protocol,
 * backed by the CDK Rust library for consistent cross-platform crypto.
 */
export interface HybridOutputDataCreator
  extends HybridObject<{ ios: 'c++'; android: 'c++' }> {
  // --- Random outputs (ephemeral secrets) ---

  createSingleRandomData(amount: number, keysetId: string): OutputData;

  createRandomData(
    amount: number,
    keysetId: string,
    keys: KeyEntry[],
    customSplit?: number[],
  ): OutputData[];

  // --- P2PK outputs (pay-to-public-key locked) ---

  createSingleP2PKData(
    p2pk: P2PKOptions,
    amount: number,
    keysetId: string,
  ): OutputData;

  createP2PKData(
    p2pk: P2PKOptions,
    amount: number,
    keysetId: string,
    keys: KeyEntry[],
    customSplit?: number[],
  ): OutputData[];

  // --- Deterministic outputs (BIP32-derived secrets) ---

  createSingleDeterministicData(
    amount: number,
    seed: ArrayBuffer,
    counter: number,
    keysetId: string,
  ): OutputData;

  createDeterministicData(
    amount: number,
    seed: ArrayBuffer,
    counter: number,
    keysetId: string,
    keys: KeyEntry[],
    customSplit?: number[],
  ): OutputData[];
}
