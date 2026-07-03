use crate::dhke::verify_bls_blind_signature;
use crate::nuts::{nut12, BlindSignature, BlindedMessage, KeySetVersion};
use crate::wallet::Wallet;
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
/// actual change or restored amount. DLEQ proofs are optional for v0/v1
/// compatibility, but when present they are verified after the signature
/// metadata has been cross-checked. V3/BLS signatures must not include DLEQ
/// proof data and are verified with BLS pairings.
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

        if sig.keyset_id != blinded_message.keyset_id {
            return Err(Error::InvalidMintResponse(format!(
                "mint signature keyset ({}) does not match requested keyset ({})",
                sig.keyset_id, blinded_message.keyset_id
            )));
        }

        let keys = wallet.load_keyset_keys(sig.keyset_id).await?;
        let key = keys.amount_key(sig.amount).ok_or(Error::AmountKey)?;

        match sig.keyset_id.get_version() {
            KeySetVersion::Version00 | KeySetVersion::Version01 => {
                match sig.verify_dleq(key, blinded_message.blinded_secret) {
                    Ok(_) | Err(nut12::Error::MissingDleqProof) => (),
                    Err(_) => return Err(Error::CouldNotVerifyDleq),
                }
            }
            KeySetVersion::Version02 => {
                if sig.dleq.is_some() {
                    return Err(Error::CouldNotVerifyDleq);
                }
                verify_bls_blind_signature(key, sig.c, blinded_message.blinded_secret)
                    .map_err(|_| Error::CouldNotVerifyDleq)?;
            }
        }
    }

    Ok(())
}
