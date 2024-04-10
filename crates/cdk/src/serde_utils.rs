//! Utilities for serde

// TODO: remove this module

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
