//! Utilities for serde

pub mod serde_url {
    use serde::Deserialize;
    use url::Url;

    pub fn serialize<S>(url: &Url, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(url.as_ref())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Url, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let url_string = String::deserialize(deserializer)?;
        Url::parse(&url_string).map_err(serde::de::Error::custom)
    }
}
