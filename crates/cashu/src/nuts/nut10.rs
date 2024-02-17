use serde::ser::SerializeTuple;
use serde::{Deserialize, Serialize, Serializer};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Kind {
    /// NUT-11 P2PK
    #[default]
    P2PK,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub struct SecretData {
    /// Unique random string
    pub nonce: String,
    /// Expresses the spending condition specific to each kind
    pub data: String,
    /// Additional data committed to and can be used for feature extensions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Vec<String>>>,
}

#[derive(Debug, Default, Clone, Deserialize, PartialEq, Eq)]
pub struct Secret {
    ///  Kind of the spending condition
    pub kind: Kind,
    pub secret_data: SecretData,
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
/*
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UncheckedSecret(String);

impl UncheckedSecret {
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Display for UncheckedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl<S> From<S> for UncheckedSecret
where
    S: Into<String>,
{
    fn from(url: S) -> Self {
        Self(url.into())
    }
}

impl FromStr for UncheckedSecret {
    type Err = ParseError;

    fn from_str(url: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(url))
    }
}

impl TryFrom<UncheckedSecret> for Secret {
    type Error = serde_json::Error;

    fn try_from(unchecked_secret: UncheckedSecret) -> Result<Secret, Self::Error> {
        serde_json::from_str(&unchecked_secret.0)
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
        let secret_str = String::from(
            r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198","tags":[["key","value1","value2"]]}]"#,
        );
        println!("{}", secret_str);

        let t = UncheckedSecret::from_str(&secret_str);

        let secret_ser: UncheckedSecret = serde_json::from_str(&secret_str).unwrap();
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

        println!("{}", serde_json::to_string(&secret).unwrap());

        assert_eq!(secret, secret_ser.try_into().unwrap());
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

        let secret = UncheckedSecret(secret_str.to_string());

        println!("se; {}", secret);

        assert_eq!(secret.to_string(), secret_str);
    }

    #[test]
    fn test_secret_roundtrip() {
        let secret_str = r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198","tags":[["key","value1","value2"]]}]"#;

        let secret_ser: Secret = serde_json::from_str(secret_str).unwrap();

        assert_eq!(serde_json::to_string(&secret_ser).unwrap(), secret_str)
    }

    #[test]
    fn test_unchecked_secret_roundtrip() {
        let secret_str = r#"["P2PK",{"nonce":"5d11913ee0f92fefdc82a6764fd2457a","data":"026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198","tags":[["key","value1","value2"]]}]"#.to_string();

        let secret_ser: UncheckedSecret = serde_json::from_str(&secret_str).unwrap();

        assert_eq!(secret_ser.to_string(), secret_str)
    }
}
*/
