//! The NUT-00 signing rule for v3 transaction inputs
//!
//! Every v3 input carries a witness satisfying its key over its input
//! digest: a proof input's key is its secret, a mint quote input's key is
//! its lock key.

use super::verify::{sign_digest, verify_witness};
use super::witness::NutrootWitness;
use super::Error;
use crate::nuts::nut00::transaction::{Transaction, TransactionInput};
use crate::nuts::nut00::{Proof, Witness};
use crate::nuts::nut01::{PublicKey, SecretKey};
use crate::nuts::nut02::KeySetVersion;

/// Verify the witness of v3 proof `proof` at transaction input `index`
///
/// The proof must be the one the input was built from; its amount, keyset
/// and signature are checked against the input.
pub fn verify_proof_input(
    transaction: &Transaction,
    index: usize,
    proof: &Proof,
    now: u64,
) -> Result<(), Error> {
    if proof.keyset_id.get_version() != KeySetVersion::Version02 {
        return Err(Error::NotV3);
    }
    match transaction.inputs().get(index) {
        Some(TransactionInput::Proof {
            amount,
            keyset_id,
            c,
            ..
        }) if *amount == proof.amount && *keyset_id == proof.keyset_id && *c == proof.c => (),
        _ => return Err(Error::InputMismatch(index)),
    }
    let secret = proof.secret.v3_point()?;
    let witness = proof
        .witness
        .as_ref()
        .ok_or(Error::MissingWitness)?
        .as_nutroot()
        .ok_or(Error::MissingWitness)?
        .parse()?;
    verify_witness(&secret, &witness, &transaction.input_digest(index)?, now)
}

/// Verify a mint quote's `signature` at transaction input `index`
///
/// `signature` is a serialized witness or a bare 128-hex key-path
/// signature.
pub fn verify_quote_input(
    transaction: &Transaction,
    index: usize,
    signature: Option<&str>,
    now: u64,
) -> Result<(), Error> {
    let Some(TransactionInput::MintQuote { lock_key, .. }) = transaction.inputs().get(index) else {
        return Err(Error::InputMismatch(index));
    };
    let witness = NutrootWitness::from_quote_signature(signature.ok_or(Error::MissingWitness)?)?;
    verify_witness(lock_key, &witness, &transaction.input_digest(index)?, now)
}

/// Sign transaction input `index` by the key path with `secret_key`
///
/// `secret_key` is `k` for a bare secret, or the tweaked `p' = k + t`.
pub fn sign_input_key_path(
    transaction: &Transaction,
    index: usize,
    secret_key: &SecretKey,
) -> Result<NutrootWitness, Error> {
    let digest = transaction.input_digest(index)?;
    Ok(NutrootWitness::key_path(sign_digest(secret_key, &digest)))
}

/// Sign every v3 proof input by the key path, attaching the witnesses
///
/// `key_for` maps a proof's secret point to its spend key; proofs it has
/// no key for are left unsigned. Pre-v3 proofs are skipped.
pub fn sign_proofs_key_path<F>(
    transaction: &Transaction,
    proofs: &mut [Proof],
    key_for: F,
) -> Result<(), Error>
where
    F: Fn(&PublicKey) -> Option<SecretKey>,
{
    for (index, proof) in proofs.iter_mut().enumerate() {
        if proof.keyset_id.get_version() != KeySetVersion::Version02 {
            continue;
        }
        let point = proof.secret.v3_point()?;
        if let Some(secret_key) = key_for(&point) {
            let witness = sign_input_key_path(transaction, index, &secret_key)?;
            proof.witness = Some(Witness::from(witness));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nuts::nut00::transaction::TransactionOutput;
    use crate::nuts::nut13::derive_v3_secret_key;
    use crate::nuts::Id;
    use crate::Amount;

    const KEYSET: &str = "022546ab0a2dee9f79510f4e20a4e3d9e894a97361047c594902573a7e64f9d701";
    const SEED_HEX: &str = "6e757431332076332074657374207365656400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

    fn swap_proof() -> Proof {
        serde_json::from_value(serde_json::json!({
            "amount": 8,
            "id": KEYSET,
            "secret": "0240c186f3e19f6f8cd00db81c5266d5c74ddca48ff848efd0e3e125b2846f30d9",
            "C": "a33dd4de26ea950060ae5c45bdc408d72aaf51ba9f4ccdeee4a040df067ca87c799a406fec88c2a328b75b290e27f7b6",
        }))
        .unwrap()
    }

    fn swap_tx(proof: &Proof) -> Transaction {
        let output: crate::nuts::BlindedMessage = serde_json::from_value(serde_json::json!({
            "amount": 8, "id": KEYSET,
            "B_": "b42a0bcc39598db1dca617aeea6bc367f2566636826dc961a54faae15b3b8d10afc1cb0206e70ab3b0e12c2b9478cd55",
        }))
        .unwrap();
        Transaction::new(
            vec![TransactionInput::from_proof(proof).unwrap()],
            vec![TransactionOutput::from_blinded_message(&output)],
        )
        .unwrap()
    }

    fn k0() -> SecretKey {
        let seed = crate::util::hex::decode(SEED_HEX).unwrap();
        derive_v3_secret_key(&seed, KEYSET.parse::<Id>().unwrap(), 0).unwrap()
    }

    #[test]
    fn signed_proof_verifies_and_round_trips() {
        let mut proofs = vec![swap_proof()];
        let tx = swap_tx(&proofs[0]);
        assert!(matches!(
            verify_proof_input(&tx, 0, &proofs[0], 0),
            Err(Error::MissingWitness)
        ));
        sign_proofs_key_path(&tx, &mut proofs, |_| Some(k0())).unwrap();
        verify_proof_input(&tx, 0, &proofs[0], 0).unwrap();

        // The witness survives JSON as a v3 nutroot witness.
        let json = serde_json::to_string(&proofs[0]).unwrap();
        let parsed: Proof = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed.witness, Some(Witness::Nutroot(_))));
        verify_proof_input(&tx, 0, &parsed, 0).unwrap();

        // Another key does not.
        let mut wrong = vec![swap_proof()];
        sign_proofs_key_path(&tx, &mut wrong, |_| Some(SecretKey::generate())).unwrap();
        assert!(matches!(
            verify_proof_input(&tx, 0, &wrong[0], 0),
            Err(Error::InvalidSignature)
        ));
    }

    #[test]
    fn input_must_match_the_proof() {
        let proof = swap_proof();
        let tx = swap_tx(&proof);
        let mut other = proof.clone();
        other.amount = Amount::from(4);
        assert!(matches!(
            verify_proof_input(&tx, 0, &other, 0),
            Err(Error::InputMismatch(0))
        ));
        assert!(matches!(
            verify_proof_input(&tx, 1, &proof, 0),
            Err(Error::InputMismatch(1))
        ));
        assert!(matches!(
            verify_quote_input(&tx, 0, Some(""), 0),
            Err(Error::InputMismatch(0))
        ));
    }

    /// tests/00-tests.md "Serialized quote witnesses"
    #[test]
    fn quote_witness_vectors() {
        let lock = |hex: &str| PublicKey::from_hex(hex).unwrap();
        let output: crate::nuts::BlindedMessage = serde_json::from_value(serde_json::json!({
            "amount": 4, "id": KEYSET,
            "B_": "b42a0bcc39598db1dca617aeea6bc367f2566636826dc961a54faae15b3b8d10afc1cb0206e70ab3b0e12c2b9478cd55",
        }))
        .unwrap();
        let partial = Transaction::new(
            vec![TransactionInput::MintQuote {
                amount: Amount::from(4),
                quote_id: "quote-mint-0004".to_string(),
                lock_key: lock(
                    "0292905560a6a511a13e383ee27e220aeffc85b7a7bc293e6935b1d3678d209812",
                ),
                issued: Amount::from(3),
            }],
            vec![TransactionOutput::from_blinded_message(&output)],
        )
        .unwrap();
        let sig = "c6709751b0d3c14fef333326f4cd9d45c7af327ea4659a7b4637e92d9a82b0f6c25a4bce9e99385f40f39ea6fe26af9ba07bfaf2c378f2f47a1eb2d474a0a7a6";
        verify_quote_input(&partial, 0, Some(sig), 0).unwrap();
        verify_quote_input(
            &partial,
            0,
            Some(&format!(r#"{{"signatures":["{sig}"]}}"#)),
            0,
        )
        .unwrap();
        assert!(matches!(
            verify_quote_input(&partial, 0, None, 0),
            Err(Error::MissingWitness)
        ));

        // Script-only quote lock spent by the script path.
        let mut output = output;
        output.amount = Amount::from(8);
        let script_only = Transaction::new(
            vec![TransactionInput::MintQuote {
                amount: Amount::from(8),
                quote_id: "quote-mint-0001".to_string(),
                lock_key: lock(
                    "034b15129a9c665f37fbe9f6c80a7fe6ec0c809f34edde98d298aff94a1e8af276",
                ),
                issued: Amount::ZERO,
            }],
            vec![TransactionOutput::from_blinded_message(&output)],
        )
        .unwrap();
        let witness = r#"{"leaf":"00010200010104002102f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9","control":{"K":"028edfebd6fdea3e1d89359af20868a2e76315b36cdb1a79de497a1757ca7bd407","path":[]},"signatures":["b0656dbde143bd6825e141551cae68f1428a62ee1d74c0f8df8f735d5841471d6f20f3946658bdad225b6b30dc90cea956a0aede132edf6eb1415f95afb30ca2"]}"#;
        verify_quote_input(&script_only, 0, Some(witness), 0).unwrap();
        // The partial mint's signature does not satisfy it.
        assert!(verify_quote_input(&script_only, 0, Some(sig), 0).is_err());
    }
}
