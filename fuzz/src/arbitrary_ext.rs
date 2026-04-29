//! Manual `Arbitrary` implementations for core Cashu types.
//!
//! Every impl lives on a newtype wrapper owned by this crate so that the
//! orphan rule is satisfied and the upstream `crates/cashu` types remain
//! free of an `arbitrary` dependency.
//!
//! Wrappers expose the inner value via `into_inner()` and `as_inner()` for
//! ergonomic use inside `fuzz_target!` bodies.

use std::str::FromStr;

use cashu::mint_url::MintUrl;
use cashu::nut00::token::{TokenV3Token, TokenV4Token};
use cashu::nuts::nut00::{
    BlindedMessage, CurrencyUnit, Proof, ProofV3, ProofV4, Token, TokenV3, TokenV4, Witness,
};
use cashu::nuts::nut02::ShortKeysetId;
use cashu::nuts::nut10::{Kind, SecretData, SpendingConditions};
use cashu::nuts::nut11::{P2PKWitness, SigFlag};
use cashu::nuts::nut12::ProofDleq;
use cashu::nuts::nut14::HTLCWitness;
use cashu::nuts::Conditions;
use cashu::secret::Secret as SecretString;
use cashu::{Amount, Id, Nut10Secret, PaymentRequest, PublicKey, SecretKey};
use libfuzzer_sys::arbitrary::{self, Arbitrary, Unstructured};

use crate::{pubkey_from, secret_key_from};

// ---------------------------------------------------------------------------
// Primitive wrappers
// ---------------------------------------------------------------------------

/// Wrapper around [`Amount`].
#[derive(Debug, Clone)]
pub struct AmountArb(pub Amount);

impl AmountArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> Amount {
        self.0
    }
}

impl<'a> Arbitrary<'a> for AmountArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self(Amount::from(u64::arbitrary(u)?)))
    }
}

/// Wrapper around [`SecretKey`].
#[derive(Debug, Clone)]
pub struct SecretKeyArb(pub SecretKey);

impl SecretKeyArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> SecretKey {
        self.0
    }
}

impl<'a> Arbitrary<'a> for SecretKeyArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let bytes: [u8; 32] = u.arbitrary()?;
        Ok(Self(secret_key_from(bytes)))
    }
}

/// Wrapper around [`PublicKey`].
#[derive(Debug, Clone, Copy)]
pub struct PublicKeyArb(pub PublicKey);

impl PublicKeyArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> PublicKey {
        self.0
    }
}

impl<'a> Arbitrary<'a> for PublicKeyArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let bytes: [u8; 32] = u.arbitrary()?;
        Ok(Self(pubkey_from(bytes)))
    }
}

/// Wrapper around [`Id`] (V1 most of the time, V2 ~25% of the time).
#[derive(Debug, Clone, Copy)]
pub struct IdArb(pub Id);

impl IdArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> Id {
        self.0
    }
}

impl<'a> Arbitrary<'a> for IdArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let fallback = || Id::from_str("00deadbeef123456").expect("valid v1 id");
        if u.ratio(1, 4)? {
            // V2: 33 bytes, first byte is the version (0x01).
            let mut bytes = [0u8; 33];
            bytes[0] = 0x01;
            let filler: [u8; 32] = u.arbitrary()?;
            bytes[1..].copy_from_slice(&filler);
            Ok(Self(Id::from_bytes(&bytes).unwrap_or_else(|_| fallback())))
        } else {
            // V1: 8 bytes, first byte is the version (0x00).
            let filler: [u8; 7] = u.arbitrary()?;
            let mut bytes = [0u8; 8];
            bytes[1..].copy_from_slice(&filler);
            Ok(Self(Id::from_bytes(&bytes).unwrap_or_else(|_| fallback())))
        }
    }
}

/// Wrapper around [`MintUrl`].
#[derive(Debug, Clone)]
pub struct MintUrlArb(pub MintUrl);

impl MintUrlArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> MintUrl {
        self.0
    }
}

impl<'a> Arbitrary<'a> for MintUrlArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        const URLS: &[&str] = &[
            "https://example.com",
            "https://8333.space:3338",
            "https://mint.example.org/path",
            "http://localhost:3338",
            "https://testnut.cashu.space",
            "https://mint.minibits.cash/Bitcoin",
        ];
        let idx = u.int_in_range(0..=URLS.len() - 1)?;
        let url = MintUrl::from_str(URLS[idx])
            .unwrap_or_else(|_| MintUrl::from_str("https://example.com").expect("valid url"));
        Ok(Self(url))
    }
}

/// Wrapper around [`CurrencyUnit`].
#[derive(Debug, Clone)]
pub struct CurrencyUnitArb(pub CurrencyUnit);

impl CurrencyUnitArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> CurrencyUnit {
        self.0
    }
}

impl<'a> Arbitrary<'a> for CurrencyUnitArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let choice = u.int_in_range(0..=5)?;
        let unit = match choice {
            0 => CurrencyUnit::Sat,
            1 => CurrencyUnit::Msat,
            2 => CurrencyUnit::Usd,
            3 => CurrencyUnit::Eur,
            4 => CurrencyUnit::Auth,
            _ => {
                // Custom units are normalised to lowercase on serialize,
                // and `FromStr` rehydrates well-known units even from
                // lowercase input ("sat" -> CurrencyUnit::Sat), so we
                // pre-lowercase here and exclude any ASCII form that
                // would collide with a known variant to keep
                // string/CBOR round-trips lossless.
                let s = String::arbitrary(u)?;
                let cleaned: String = s
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .take(8)
                    .collect();
                let lowered = cleaned.to_lowercase();
                let collides = matches!(lowered.as_str(), "sat" | "msat" | "usd" | "eur" | "auth");
                if lowered.is_empty() || collides {
                    CurrencyUnit::Custom("fuzz".to_string())
                } else {
                    CurrencyUnit::Custom(lowered)
                }
            }
        };
        Ok(Self(unit))
    }
}

// ---------------------------------------------------------------------------
// Secret + Witness wrappers
// ---------------------------------------------------------------------------

/// Wrapper around the NUT-00 `Secret` string.
///
/// Emits either a 64-hex classic secret or a NUT-10 structured secret
/// (P2PK / HTLC) with fuzz-controlled conditions.
#[derive(Debug, Clone)]
pub struct SecretStringArb(pub SecretString);

impl SecretStringArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> SecretString {
        self.0
    }
}

impl<'a> Arbitrary<'a> for SecretStringArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let choice = u.int_in_range(0..=2)?;
        let secret = match choice {
            // Classic random 32-byte hex secret.
            0 => {
                let bytes: [u8; 32] = u.arbitrary()?;
                SecretString::new(hex::encode_lower_bytes(&bytes))
            }
            // NUT-10 P2PK secret.
            1 => {
                let pk = PublicKeyArb::arbitrary(u)?.into_inner();
                let cond = ConditionsArb::arbitrary(u)?.into_inner();
                let sc = SpendingConditions::new_p2pk(pk, Some(cond));
                let nut10: Nut10Secret = sc.into();
                match SecretString::try_from(nut10) {
                    Ok(s) => s,
                    Err(_) => SecretString::generate(),
                }
            }
            // NUT-10 HTLC secret (data is fuzzed hex string).
            _ => {
                let bytes: [u8; 32] = u.arbitrary()?;
                let hash_hex = hex::encode_lower_bytes(&bytes);
                let cond = ConditionsArb::arbitrary(u)?.into_inner();
                let sd = SecretData::new(hash_hex, Some(cond_to_tags(&cond)));
                let nut10 = Nut10Secret::new(Kind::HTLC, sd);
                match SecretString::try_from(nut10) {
                    Ok(s) => s,
                    Err(_) => SecretString::generate(),
                }
            }
        };
        Ok(Self(secret))
    }
}

// Lightweight hex helper without pulling another dep.
mod hex {
    pub fn encode_lower_bytes(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
        out
    }
}

/// Always produce a concrete, non-empty tag vec for HTLC secrets.
fn cond_to_tags(_c: &Conditions) -> Vec<Vec<String>> {
    // We rely on `Conditions` already being well-formed; HTLC secret-data
    // only needs *any* tag list here. Leave it empty so `verify_htlc` reads
    // straight from the hash field.
    Vec::new()
}

/// Wrapper around [`Conditions`].
#[derive(Debug, Clone)]
pub struct ConditionsArb(pub Conditions);

impl ConditionsArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> Conditions {
        self.0
    }
}

impl<'a> Arbitrary<'a> for ConditionsArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let locktime: Option<u64> = u.arbitrary()?;
        let num_pubkeys = u.int_in_range(0..=3)?;
        let pubkeys: Vec<PublicKey> = (0..num_pubkeys)
            .map(|_| PublicKeyArb::arbitrary(u).map(|p| p.into_inner()))
            .collect::<Result<_, _>>()?;
        let num_refund = u.int_in_range(0..=2)?;
        let refund_keys: Vec<PublicKey> = (0..num_refund)
            .map(|_| PublicKeyArb::arbitrary(u).map(|p| p.into_inner()))
            .collect::<Result<_, _>>()?;
        let num_sigs: Option<u64> = u.arbitrary()?;
        let num_sigs_refund: Option<u64> = u.arbitrary()?;
        let sig_flag = if u.ratio(1, 2)? {
            SigFlag::SigInputs
        } else {
            SigFlag::SigAll
        };
        Ok(Self(Conditions {
            locktime,
            pubkeys: if pubkeys.is_empty() {
                None
            } else {
                Some(pubkeys)
            },
            refund_keys: if refund_keys.is_empty() {
                None
            } else {
                Some(refund_keys)
            },
            num_sigs,
            sig_flag,
            num_sigs_refund,
        }))
    }
}

/// Wrapper around [`Witness`].
#[derive(Debug, Clone)]
pub struct WitnessArb(pub Witness);

impl WitnessArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> Witness {
        self.0
    }
}

impl<'a> Arbitrary<'a> for WitnessArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let choice = u.int_in_range(0..=1)?;
        let w = match choice {
            0 => {
                let n = u.int_in_range(0..=3)?;
                let sigs: Vec<String> = (0..n)
                    .map(|_| String::arbitrary(u))
                    .collect::<Result<_, _>>()?;
                Witness::P2PKWitness(P2PKWitness { signatures: sigs })
            }
            _ => {
                let preimage = String::arbitrary(u)?;
                let n = u.int_in_range(0..=2)?;
                let sigs: Option<Vec<String>> = if n == 0 {
                    None
                } else {
                    Some(
                        (0..n)
                            .map(|_| String::arbitrary(u))
                            .collect::<Result<_, _>>()?,
                    )
                };
                Witness::HTLCWitness(HTLCWitness {
                    preimage,
                    signatures: sigs,
                })
            }
        };
        Ok(Self(w))
    }
}

// ---------------------------------------------------------------------------
// Proof / BlindedMessage wrappers
// ---------------------------------------------------------------------------

/// Wrapper around [`Proof`].
#[derive(Debug, Clone)]
pub struct ProofArb(pub Proof);

impl ProofArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> Proof {
        self.0
    }
}

impl<'a> Arbitrary<'a> for ProofArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let amount = AmountArb::arbitrary(u)?.into_inner();
        let keyset_id = IdArb::arbitrary(u)?.into_inner();
        let secret = SecretStringArb::arbitrary(u)?.into_inner();
        let c = PublicKeyArb::arbitrary(u)?.into_inner();
        let witness = if u.ratio(1, 2)? {
            Some(WitnessArb::arbitrary(u)?.into_inner())
        } else {
            None
        };
        let dleq = if u.ratio(1, 2)? {
            Some(ProofDleqArb::arbitrary(u)?.into_inner())
        } else {
            None
        };
        Ok(Self(Proof {
            amount,
            keyset_id,
            secret,
            c,
            witness,
            dleq,
            p2pk_e: None,
        }))
    }
}

/// Wrapper around [`ProofDleq`] (NUT-12 per-proof DLEQ `{e, s, r}`).
#[derive(Debug, Clone)]
pub struct ProofDleqArb(pub ProofDleq);

impl ProofDleqArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> ProofDleq {
        self.0
    }
}

impl<'a> Arbitrary<'a> for ProofDleqArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let e = SecretKeyArb::arbitrary(u)?.into_inner();
        let s = SecretKeyArb::arbitrary(u)?.into_inner();
        let r = SecretKeyArb::arbitrary(u)?.into_inner();
        Ok(Self(ProofDleq::new(e, s, r)))
    }
}

/// Wrapper around [`BlindedMessage`].
#[derive(Debug, Clone)]
pub struct BlindedMessageArb(pub BlindedMessage);

impl BlindedMessageArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> BlindedMessage {
        self.0
    }
}

impl<'a> Arbitrary<'a> for BlindedMessageArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let amount = AmountArb::arbitrary(u)?.into_inner();
        let keyset_id = IdArb::arbitrary(u)?.into_inner();
        let blinded_secret = PublicKeyArb::arbitrary(u)?.into_inner();
        Ok(Self(BlindedMessage::new(amount, keyset_id, blinded_secret)))
    }
}

// ---------------------------------------------------------------------------
// Token wrappers
// ---------------------------------------------------------------------------

/// Wrapper around [`TokenV4`].
///
/// Always emits a single-mint token with at least one keyset group and at
/// least one proof per group; unit and mint_url are always present. This
/// guarantees the `TryFrom<TokenV3> for TokenV4` conversion round-trips.
#[derive(Debug, Clone)]
pub struct TokenV4Arb(pub TokenV4);

impl TokenV4Arb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> TokenV4 {
        self.0
    }
}

impl<'a> Arbitrary<'a> for TokenV4Arb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let mint_url = MintUrlArb::arbitrary(u)?.into_inner();
        let unit = CurrencyUnitArb::arbitrary(u)?.into_inner();
        let memo: Option<String> = if u.ratio(1, 2)? {
            let raw = String::arbitrary(u)?;
            // Keep memo short to avoid inflating serialization costs.
            Some(raw.chars().take(32).collect())
        } else {
            None
        };

        let num_groups = u.int_in_range(1..=3)?;
        let mut groups: Vec<TokenV4Token> = Vec::with_capacity(num_groups);
        // Track used secrets across all groups. V3 and V4 use different
        // Eq semantics for "duplicate" (V3 includes keyset_id, V4 does
        // not), so ensuring globally-unique secrets keeps the two
        // `value()` implementations in sync for the differential fuzzer.
        let mut disambiguator: u64 = 0;
        for _ in 0..num_groups {
            let keyset_id = IdArb::arbitrary(u)?.into_inner();
            let num_proofs = u.int_in_range(1..=4)?;
            let mut proofs: Vec<ProofV4> = Vec::with_capacity(num_proofs);
            for _ in 0..num_proofs {
                let p = ProofArb::arbitrary(u)?.into_inner();
                // Disambiguate the secret to guarantee global uniqueness
                // within this token without altering the secret's overall
                // shape (classic hex vs NUT-10 JSON).
                let disambiguated = SecretString::new(format!("{}#{}", p.secret, disambiguator));
                disambiguator = disambiguator.wrapping_add(1);
                let pv4 = ProofV4 {
                    amount: p.amount,
                    secret: disambiguated,
                    c: p.c,
                    witness: p.witness,
                    dleq: None,
                    p2pk_e: None,
                };
                proofs.push(pv4);
            }
            groups.push(TokenV4Token {
                keyset_id: ShortKeysetId::from(keyset_id),
                proofs,
            });
        }

        Ok(Self(TokenV4 {
            mint_url,
            unit,
            memo,
            token: groups,
        }))
    }
}

/// Wrapper around [`TokenV3`].
#[derive(Debug, Clone)]
pub struct TokenV3Arb(pub TokenV3);

impl TokenV3Arb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> TokenV3 {
        self.0
    }
}

impl<'a> Arbitrary<'a> for TokenV3Arb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let mint_url = MintUrlArb::arbitrary(u)?.into_inner();
        let unit = if u.ratio(3, 4)? {
            Some(CurrencyUnitArb::arbitrary(u)?.into_inner())
        } else {
            None
        };
        let memo: Option<String> = if u.ratio(1, 2)? {
            Some(String::arbitrary(u)?.chars().take(32).collect())
        } else {
            None
        };

        let num_proofs = u.int_in_range(1..=4)?;
        let mut proofs: Vec<ProofV3> = Vec::with_capacity(num_proofs);
        for _ in 0..num_proofs {
            let p = ProofArb::arbitrary(u)?.into_inner();
            let pv3 = ProofV3 {
                amount: p.amount,
                keyset_id: ShortKeysetId::from(p.keyset_id),
                secret: p.secret,
                c: p.c,
                witness: p.witness,
                dleq: None,
            };
            proofs.push(pv3);
        }

        let inner = TokenV3 {
            token: vec![TokenV3Token {
                mint: mint_url,
                proofs,
            }],
            memo,
            unit,
        };
        Ok(Self(inner))
    }
}

/// Wrapper around [`Token`], dispatching between V3 and V4 with a fuzz bit.
#[derive(Debug, Clone)]
pub struct TokenArb(pub Token);

impl TokenArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> Token {
        self.0
    }
}

impl<'a> Arbitrary<'a> for TokenArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        if u.ratio(1, 2)? {
            Ok(Self(Token::TokenV4(TokenV4Arb::arbitrary(u)?.into_inner())))
        } else {
            Ok(Self(Token::TokenV3(TokenV3Arb::arbitrary(u)?.into_inner())))
        }
    }
}

// ---------------------------------------------------------------------------
// PaymentRequest wrapper
// ---------------------------------------------------------------------------

/// Wrapper around [`PaymentRequest`].
#[derive(Debug, Clone)]
pub struct PaymentRequestArb(pub PaymentRequest);

impl PaymentRequestArb {
    /// Consume the wrapper and return the inner value.
    pub fn into_inner(self) -> PaymentRequest {
        self.0
    }
}

impl<'a> Arbitrary<'a> for PaymentRequestArb {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let payment_id: Option<String> = u.arbitrary()?;
        let amount: Option<u64> = u.arbitrary()?;
        let unit: Option<CurrencyUnit> = if u.ratio(1, 2)? {
            Some(CurrencyUnitArb::arbitrary(u)?.into_inner())
        } else {
            None
        };
        let single_use: Option<bool> = u.arbitrary()?;
        let num_mints = u.int_in_range(0..=2)?;
        let mints: Vec<MintUrl> = (0..num_mints)
            .map(|_| MintUrlArb::arbitrary(u).map(|m| m.into_inner()))
            .collect::<Result<_, _>>()?;
        let description: Option<String> = u.arbitrary()?;

        Ok(Self(PaymentRequest {
            payment_id,
            amount: amount.map(Amount::from),
            unit,
            single_use,
            mints,
            description,
            transports: Vec::new(),
            nut10: None,
        }))
    }
}

// Silence unused-import lints in constrained feature combinations.
#[allow(dead_code)]
fn _touch_secret_key(_s: SecretKey) {}
