use std::collections::hash_map::Entry;
use std::collections::HashMap;

use cdk_common::dhke::construct_proofs;

use crate::nuts::SecretKey;
use crate::nuts::{nut12, BlindSignature, BlindedMessage, Proofs};
use crate::secret::Secret;
use crate::wallet::{KeysetLoadPolicy, Wallet};
use crate::{Amount, Error};

/// How strictly returned signature amounts must match requested output amounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignatureAmountValidation {
    /// Returned signature amounts must exactly match the requested output amounts.
    Exact,
    /// A zero-amount requested output is a placeholder and may receive any amount.
    AllowZeroAmountPlaceholder,
}

/// Validate mint-returned blind signatures against the wallet's requested outputs.
///
/// The mint controls the `amount` and `keyset_id` fields in each returned
/// [`BlindSignature`], so callers must verify those fields against the
/// corresponding premint blinded message before constructing wallet proofs.
///
/// Use [`SignatureAmountValidation::Exact`] for mint/swap responses where the
/// wallet requested a specific denomination. Use
/// [`SignatureAmountValidation::AllowZeroAmountPlaceholder`] for NUT-08/NUT-09
/// style outputs where the wallet sends amount `0` and the mint fills in the
/// actual change or restored amount. DLEQ proofs are optional for compatibility,
/// but when present they are verified after the signature metadata has been
/// cross-checked.
pub(crate) async fn validate_mint_response_signatures<'a>(
    wallet: &Wallet,
    signatures: &[BlindSignature],
    blinded_messages: impl IntoIterator<Item = &'a BlindedMessage>,
    amount_validation: SignatureAmountValidation,
) -> Result<(), Error> {
    let blinded_messages = blinded_messages.into_iter().collect::<Vec<_>>();

    if signatures.len() != blinded_messages.len() {
        return Err(Error::InvalidMintResponse(format!(
            "mint signatures ({}) does not match secrets sent ({})",
            signatures.len(),
            blinded_messages.len()
        )));
    }

    for (sig, blinded_message) in signatures.iter().zip(blinded_messages) {
        let amount_matches = match amount_validation {
            SignatureAmountValidation::Exact => sig.amount == blinded_message.amount,
            SignatureAmountValidation::AllowZeroAmountPlaceholder => {
                blinded_message.amount == Amount::ZERO || sig.amount == blinded_message.amount
            }
        };

        if !amount_matches {
            return Err(Error::InvalidMintResponse(format!(
                "mint signature amount ({}) does not match requested amount ({})",
                sig.amount, blinded_message.amount
            )));
        }

        // A zero-amount blank may come back signed on another keyset of this unit (NUT-08).
        let mismatch = || {
            Error::InvalidMintResponse(format!(
                "mint signature keyset ({}) does not match requested keyset ({})",
                sig.keyset_id, blinded_message.keyset_id
            ))
        };
        let other_keyset = sig.keyset_id != blinded_message.keyset_id;
        let placeholder = amount_validation
            == SignatureAmountValidation::AllowZeroAmountPlaceholder
            && blinded_message.amount == Amount::ZERO;
        if other_keyset && !placeholder {
            return Err(mismatch());
        }

        // `keyset` only finds keysets of this unit, so a foreign one is still a mismatch.
        let keys = match wallet.keyset(sig.keyset_id).await {
            Ok(keyset) => keyset.keys,
            Err(Error::UnknownKeySet) if other_keyset => return Err(mismatch()),
            Err(err) => return Err(err),
        };
        let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;
        match sig.verify_dleq(key, blinded_message.blinded_secret) {
            Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
            Err(_) => return Err(Error::CouldNotVerifyDleq),
        }
    }

    Ok(())
}

/// Unblind each validated signature with the keys of the keyset it names.
pub(crate) async fn construct_proofs_with_signing_keys(
    wallet: &Wallet,
    signatures: Vec<BlindSignature>,
    rs: Vec<SecretKey>,
    secrets: Vec<Secret>,
    policy: KeysetLoadPolicy,
) -> Result<Proofs, Error> {
    if signatures.len() != rs.len() || signatures.len() != secrets.len() {
        return Err(Error::InvalidMintResponse(format!(
            "mint signatures ({}) does not match secrets sent ({})",
            signatures.len(),
            secrets.len()
        )));
    }
    let mut keys = HashMap::new();
    for sig in &signatures {
        if let Entry::Vacant(entry) = keys.entry(sig.keyset_id) {
            entry.insert(wallet.keyset_with_policy(sig.keyset_id, policy).await?.keys);
        }
    }
    let mut proofs = Proofs::with_capacity(signatures.len());
    for ((sig, r), secret) in signatures.into_iter().zip(rs).zip(secrets) {
        let sig_keys = &keys[&sig.keyset_id];
        proofs.extend(construct_proofs(
            vec![sig],
            vec![r],
            vec![secret],
            sig_keys,
        )?);
    }
    Ok(proofs)
}
