use dlc::secp256k1_zkp::hashes::hex::FromHex;
use dlc_messages::oracle_msgs::{OracleAnnouncement, OracleAttestation};
use lightning::util::ser::Readable;
use nostr_sdk::base64::prelude::*;
use std::io::Cursor;

fn decode_bytes(str: &str) -> Result<Vec<u8>, nostr_sdk::base64::DecodeError> {
    match FromHex::from_hex(str) {
        Ok(bytes) => Ok(bytes),
        Err(_) => Ok(BASE64_STANDARD.decode(str)?),
    }
}

/// Parses a string into an oracle announcement.
pub fn oracle_announcement_from_str(str: &str) -> OracleAnnouncement {
    let bytes = decode_bytes(str).expect("Could not decode oracle announcement string");
    let mut cursor = Cursor::new(bytes);

    OracleAnnouncement::read(&mut cursor).expect("Could not parse oracle announcement")
}

/// Parses a string into an oracle attestation.
pub fn oracle_attestation_from_str(str: &str) -> OracleAttestation {
    let bytes = decode_bytes(str).expect("Could not decode oracle attestation string");
    let mut cursor = Cursor::new(bytes);

    OracleAttestation::read(&mut cursor).expect("Could not parse oracle attestation")
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use dlc::secp256k1_zkp::schnorr::Signature;
    use dlc_messages::oracle_msgs::EventDescriptor;

    const ANNOUNCEMENT: &str = "ypyyyX6pdZUM+OovHftxK9StImd8F7nxmr/eTeyR/5koOVVe/EaNw1MAeJm8LKDV1w74Fr+UJ+83bVP3ynNmjwKbtJr9eP5ie2Exmeod7kw4uNsuXcw6tqJF1FXH3fTF/dgiOwAByEOAEd95715DKrSLVdN/7cGtOlSRTQ0/LsW/p3BiVOdlpccA/dgGDAACBDEyMzQENDU2NwR0ZXN0";

    #[test]
    fn test_decode_oracle_announcement() {
        let announcement = oracle_announcement_from_str(ANNOUNCEMENT);
        println!("{:?}", announcement);

        assert_eq!(
            announcement.announcement_signature,
            Signature::from_str(&String::from("ca9cb2c97ea975950cf8ea2f1dfb712bd4ad22677c17b9f19abfde4dec91ff992839555efc468dc353007899bc2ca0d5d70ef816bf9427ef376d53f7ca73668f")).unwrap()
        );

        let descriptor = announcement.oracle_event.event_descriptor;

        match descriptor {
            EventDescriptor::EnumEvent(e) => {
                assert_eq!(e.outcomes.len(), 2);
            }
            EventDescriptor::DigitDecompositionEvent(..) => unreachable!(),
        }
    }

    #[test]
    fn test_decode_oracle_attestation() {
        let attestation = "f1d822d1b8bdddcfb07ea2890c11fb5682af346140cb9282365b0e4db950b6370001935e4441edce5bce4970b306bcb90f887a5dc0e01296869c988f83b2026b34efc3ce0d8cebda6af9338c7dbb46d2f47e2c131cff58926e2254d67b12979c48010001086f7574636f6d6531";
        let attestation = oracle_attestation_from_str(attestation);

        assert!(attestation.signatures.len() == 1);
        assert!(attestation.outcomes.len() == 1);
    }
}
