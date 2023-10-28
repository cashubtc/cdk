use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize, Serializer};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    #[default]
    P2PK,
}

impl ToString for Kind {
    fn to_string(&self) -> String {
        match self {
            Self::P2PK => "P2PK".to_string(),
        }
    }
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct SecretData {
    nonce: String,
    data: String,
    tags: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
pub struct Secret {
    kind: Kind,
    secret_data: SecretData,
}

impl Serialize for Secret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Create a tuple representing the struct fields
        let secret_tuple = (&self.kind, &self.secret_data);

        // Serialize the tuple as a JSON array
        let mut s = serializer.serialize_tuple(2)?;

        s.serialize_element(&secret_tuple.0)?;
        s.serialize_element(&secret_tuple.1)?;
        s.end()
    }
}

#[cfg(test)]
mod tests {
    use std::assert_eq;

    use super::*;

    #[test]
    fn test_secret_deserialize() {
        let secret_str = r#"[
  "P2PK",
  {
    "nonce": "5d11913ee0f92fefdc82a6764fd2457a",
    "data": "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
    "tags": [["key", "value1", "value2"]]
  }
]"#
        .to_string();

        let secret_ser: Secret = serde_json::from_str(&secret_str).unwrap();
        let secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: Some(vec![vec![
                    "key".to_string(),
                    "value1".to_string(),
                    "value2".to_string(),
                ]]),
            },
        };

        assert_eq!(secret, secret_ser);
    }

    #[test]
    fn test_secret_serialize() {
        let secret = Secret {
            kind: Kind::P2PK,
            secret_data: SecretData {
                nonce: "5d11913ee0f92fefdc82a6764fd2457a".to_string(),
                data: "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198"
                    .to_string(),
                tags: Some(vec![vec![
                    "key".to_string(),
                    "value1".to_string(),
                    "value2".to_string(),
                ]]),
            },
        };

        let secret_str = r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198","tags":[["key","value1","value2"]]}]"#;

        assert_eq!(serde_json::to_string(&secret).unwrap(), secret_str);
    }
}
