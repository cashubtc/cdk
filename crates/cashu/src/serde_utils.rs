//! Utilities for serde

pub mod serde_url {
    use serde::Deserialize;
    use url::Url;

    pub fn serialize<S>(url: &Url, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(url.to_string().trim_end_matches('/'))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Url, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let url_string = String::deserialize(deserializer)?;
        Url::parse(&url_string).map_err(serde::de::Error::custom)
    }
}

pub mod bytes_base64 {
    use base64::engine::general_purpose;
    use base64::Engine as _;
    use serde::Deserialize;

    pub fn serialize<S>(my_bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let encoded = general_purpose::STANDARD.encode(my_bytes);
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = general_purpose::STANDARD
            .decode(encoded)
            .map_err(serde::de::Error::custom)?;
        Ok(decoded)
    }
}

pub mod serde_public_key {
    use k256::PublicKey;
    use serde::Deserialize;

    pub fn serialize<S>(pubkey: &PublicKey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let encoded = hex::encode(pubkey.to_sec1_bytes());
        serializer.serialize_str(&encoded)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PublicKey, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        let decoded = hex::decode(encoded).map_err(serde::de::Error::custom)?;
        PublicKey::from_sec1_bytes(&decoded).map_err(serde::de::Error::custom)
    }

    pub mod opt {
        use k256::PublicKey;
        use serde::{Deserialize, Deserializer};

        pub fn serialize<S>(pubkey: &Option<PublicKey>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            match pubkey {
                Some(pubkey) => {
                    let encoded = hex::encode(pubkey.to_sec1_bytes());
                    serializer.serialize_str(&encoded)
                }
                None => serializer.serialize_none(),
            }
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<PublicKey>, D::Error>
        where
            D: Deserializer<'de>,
        {
            let option_str: Option<String> = Option::deserialize(deserializer)?;

            match option_str {
                Some(encoded) => {
                    let bytes = hex::decode(encoded).map_err(serde::de::Error::custom)?;
                    let pubkey =
                        PublicKey::from_sec1_bytes(&bytes).map_err(serde::de::Error::custom)?;
                    Ok(Some(pubkey))
                }
                None => Ok(None),
            }
        }
    }
}

pub mod serde_secret_key {
    use k256::SecretKey;

    pub fn serialize<S>(seckey: &SecretKey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let encoded = hex::encode(seckey.to_bytes());
        serializer.serialize_str(&encoded)
    }
    /*
        pub fn deserialize<'de, D>(deserializer: D) -> Result<SecretKey, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            let encoded = String::deserialize(deserializer)?;
            let decoded = hex::decode(encoded).map_err(serde::de::Error::custom)?;
            SecretKey::from_slice(&decoded).map_err(serde::de::Error::custom)
        }
    */
}
