//! Pay to Public Key (P2PK)
// https://github.com/cashubtc/nuts/blob/main/11.md

use serde::{Deserialize, Serialize};

use super::nut01::PublicKey;
use super::nut02::Id;
use super::nut10::Secret;
use crate::Amount;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signatures {
    signatures: Vec<String>,
}

/// Proofs [NUT-11]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    /// Amount in satoshi
    pub amount: Amount,
    /// NUT-10 Secret
    pub secret: Secret,
    /// Unblinded signature
    #[serde(rename = "C")]
    pub c: PublicKey,
    /// `Keyset id`
    pub id: Option<Id>,
    /// Witness
    pub witness: Vec<Signatures>,
}

#[cfg(test)]
mod tests {

    use std::assert_eq;

    use super::*;
    use crate::nuts::nut10::{Kind, SecretData};

    #[test]
    fn test_proof_serialize() {
        let proof = r#"[{"id":"DSAl9nvvyfva","amount":8,"C":"02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7","secret":["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"}],"witness":{"signatures":["c43d0090be59340a6364dc1340876211f2173d6a21c391115adf097adb6ea0a3ddbe7fd81b4677281decc77be09c0359faa77416025130e487f8b9169eb0c609"]}}"#;

        let proof: Proof = serde_json::from_str(proof).unwrap();

        assert_eq!(
            proof.clone().id.unwrap(),
            Id::try_from_base64("DSAl9nvvyfva").unwrap()
        );
    }

    #[test]
    fn test_proof_serualize() {
        let secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: None,
            },
        };

        let proof = Proof {
            amount: Amount::from_sat(8),
            secret,
            c: PublicKey::from_hex("02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7".to_string()).unwrap(),
            id: Some(Id::try_from_base64("DSAl9nvvyfva").unwrap()),
            witness: vec![Signatures {
                signatures: vec!["c43d0090be59340a6364dc1340876211f2173d6a21c391115adf097adb6ea0a3ddbe7fd81b4677281decc77be09c0359faa77416025130e487f8b9169eb0c609".to_string()]
            }]

            };

        let proof_str = r#"{"amount":8,"secret":["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"}],"C":"02ac910bef28cbe5d7325415d5c263026f15f9b967a079ca9779ab6e5c2db133a7","id":"DSAl9nvvyfva","witness":[{"signatures":["c43d0090be59340a6364dc1340876211f2173d6a21c391115adf097adb6ea0a3ddbe7fd81b4677281decc77be09c0359faa77416025130e487f8b9169eb0c609"]}]}"#;

        assert_eq!(serde_json::to_string(&proof).unwrap(), proof_str);
    }
}
