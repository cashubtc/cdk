//! Quote ID Lookup

use serde::{Deserialize, Serialize};

use super::PublicKey;

/// NUT-XX Settings
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct Settings {
    /// Whether the quote lookup API is available
    pub supported: bool,
}

impl Settings {
    /// Create new [`Settings`]
    pub fn new(supported: bool) -> Self {
        Self { supported }
    }
}

/// Mint quote lookup request [NUT-XX]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct PostMintQuoteLookupRequest {
    /// Public keys to lookup quotes for
    pub pubkeys: Vec<PublicKey>,
}

/// Mint quote lookup response [NUT-XX]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct PostMintQuoteLookupResponse {
    /// Matching quotes
    pub quotes: Vec<MintQuoteLookupItem>,
}

/// Individual mint quote lookup item [NUT-XX]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "swagger", derive(utoipa::ToSchema))]
pub struct MintQuoteLookupItem {
    /// Public key associated with this quote
    pub pubkey: PublicKey,
    /// Quote ID
    pub quote: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_post_mint_quote_lookup_request_serialization() {
        let pubkey1 = PublicKey::from_hex(
            "031a02b9355b1df74574ca1a85ee96f2a8cad9d650aacbec26734f9ba7309b07b2",
        )
        .unwrap();
        let pubkey2 = PublicKey::from_hex(
            "038d4f72043ca8ccb7dfb62b351e7589ca34b58ffa069834ecb0f069e3e1504c24",
        )
        .unwrap();

        let request = PostMintQuoteLookupRequest {
            pubkeys: vec![pubkey1, pubkey2],
        };

        let json = serde_json::to_string(&request).unwrap();
        let deserialized: PostMintQuoteLookupRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(request, deserialized);
        assert_eq!(deserialized.pubkeys.len(), 2);
        assert_eq!(deserialized.pubkeys[0], pubkey1);
        assert_eq!(deserialized.pubkeys[1], pubkey2);
    }

    #[test]
    fn test_post_mint_quote_lookup_response_serialization() {
        let pubkey = PublicKey::from_hex(
            "031a02b9355b1df74574ca1a85ee96f2a8cad9d650aacbec26734f9ba7309b07b2",
        )
        .unwrap();

        let lookup_item = MintQuoteLookupItem {
            pubkey,
            quote: "85233cdc-02ea-45e6-b96f-dd6dad19d28e".to_string(),
        };

        let response = PostMintQuoteLookupResponse {
            quotes: vec![lookup_item.clone()],
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: PostMintQuoteLookupResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(response, deserialized);
        assert_eq!(deserialized.quotes.len(), 1);
        assert_eq!(deserialized.quotes[0], lookup_item);
    }

    #[test]
    fn test_mint_quote_lookup_item_serialization() {
        let pubkey = PublicKey::from_hex(
            "031a02b9355b1df74574ca1a85ee96f2a8cad9d650aacbec26734f9ba7309b07b2",
        )
        .unwrap();

        let item = MintQuoteLookupItem {
            pubkey,
            quote: "85233cdc-02ea-45e6-b96f-dd6dad19d28e".to_string(),
        };

        let json = serde_json::to_string(&item).unwrap();
        let expected_json = r#"{"pubkey":"031a02b9355b1df74574ca1a85ee96f2a8cad9d650aacbec26734f9ba7309b07b2","quote":"85233cdc-02ea-45e6-b96f-dd6dad19d28e"}"#;

        assert_eq!(json, expected_json);

        let deserialized: MintQuoteLookupItem = serde_json::from_str(&json).unwrap();
        assert_eq!(item, deserialized);
    }

    #[test]
    fn test_empty_pubkeys_request() {
        let request = PostMintQuoteLookupRequest { pubkeys: vec![] };

        let json = serde_json::to_string(&request).unwrap();
        let expected_json = r#"{"pubkeys":[]}"#;

        assert_eq!(json, expected_json);

        let deserialized: PostMintQuoteLookupRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(request, deserialized);
        assert!(deserialized.pubkeys.is_empty());
    }

    #[test]
    fn test_empty_quotes_response() {
        let response = PostMintQuoteLookupResponse { quotes: vec![] };

        let json = serde_json::to_string(&response).unwrap();
        let expected_json = r#"{"quotes":[]}"#;

        assert_eq!(json, expected_json);

        let deserialized: PostMintQuoteLookupResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(response, deserialized);
        assert!(deserialized.quotes.is_empty());
    }
}
