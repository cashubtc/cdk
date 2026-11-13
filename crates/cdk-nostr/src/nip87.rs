//! NIP-87 Cashu mint discovery announcements.
//!
//! Build from the effective `/v1/info` response, then sign with a stable Nostr
//! identity and publish using the re-exported SDK. The signing identity is
//! independent of the Cashu mint public key in the `d` tag.

use cashu::nuts::MintInfo;
use nostr::prelude::{EventBuilder, Kind, Metadata, Tag, Url};
use thiserror::Error;

/// Invalid mint announcement data.
#[derive(Debug, Error)]
pub enum Error {
    /// The mint must expose a public key in `/v1/info`.
    #[error("mint discovery requires a mint public key")]
    MissingPublicKey,
    /// The URL must use HTTP(S), without credentials, a query or a fragment.
    #[error("mint discovery requires an HTTP(S) URL without credentials, query or fragment")]
    InvalidUrl,
    /// NIP-87 only defines these four networks.
    #[error("mint discovery network must be mainnet, testnet, signet or regtest")]
    InvalidNetwork,
}

/// Build a NIP-87 kind `38172` announcement from a mint's effective metadata.
///
/// Baseline NUTs 1, 2, 3 and 6 are always included. Optional capabilities are
/// advertised only when enabled in `info`. The network must be explicitly
/// supplied as `mainnet`, `testnet`, `signet` or `regtest`.
///
/// The caller owns signing, relay selection and retry policy. Reuse the signed
/// event when retrying delivery; NIP-87 does not require periodic publication.
pub fn mint_announcement(
    mint_url: &str,
    info: &MintInfo,
    network: &str,
) -> Result<EventBuilder, Error> {
    validate_announcement(mint_url, network)?;
    let pubkey = info.pubkey.ok_or(Error::MissingPublicKey)?;

    let nuts = &info.nuts;
    let mut supported = vec![1, 2, 3, 6];
    for (number, enabled) in [
        (4, !nuts.nut04.disabled && !nuts.nut04.methods.is_empty()),
        (5, !nuts.nut05.disabled && !nuts.nut05.methods.is_empty()),
        (7, nuts.nut07.supported),
        (8, nuts.nut08.supported),
        (9, nuts.nut09.supported),
        (10, nuts.nut10.supported),
        (11, nuts.nut11.supported),
        (12, nuts.nut12.supported),
        (14, nuts.nut14.supported),
        (15, !nuts.nut15.is_empty()),
        (17, !nuts.nut17.supported.is_empty()),
        (19, !nuts.nut19.cached_endpoints.is_empty()),
        (20, nuts.nut20.supported),
        (21, nuts.nut21.is_some()),
        (22, nuts.nut22.is_some()),
        (29, !nuts.nut29.is_empty()),
    ] {
        if enabled {
            supported.push(number);
        }
    }
    supported.sort_unstable();
    let supported = supported
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let metadata = Metadata {
        name: info.name.clone(),
        about: info.description_long.clone().or(info.description.clone()),
        picture: info.icon_url.clone(),
        website: Some(mint_url.to_owned()),
        ..Default::default()
    };

    Ok(
        EventBuilder::new(Kind::Custom(38172), metadata.as_json()).tags([
            Tag::identifier(pubkey.to_string()),
            Tag::custom("u", [mint_url]),
            Tag::custom("nuts", [supported]),
            Tag::custom("n", [network]),
        ]),
    )
}

/// Validate announcement connection settings before a mint identity is available.
pub fn validate_announcement(mint_url: &str, network: &str) -> Result<(), Error> {
    let url = Url::parse(mint_url).map_err(|_| Error::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::InvalidUrl);
    }
    if !matches!(network, "mainnet" | "testnet" | "signet" | "regtest") {
        return Err(Error::InvalidNetwork);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use cashu::nuts::{nut04, nut05, CurrencyUnit, PaymentMethod, PublicKey};
    use nostr::prelude::{FinalizeEvent, Keys};

    use super::*;

    fn info() -> MintInfo {
        MintInfo::new().name("Example mint").pubkey(
            PublicKey::from_hex(
                "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            )
            .expect("public key"),
        )
    }

    #[test]
    fn signs_spec_tags_with_separate_nostr_identity() {
        let info = info();
        let keys = Keys::generate();
        let event = mint_announcement("https://mint.example/cashu", &info, "regtest")
            .expect("announcement")
            .finalize(&keys)
            .expect("signed event");
        event.verify().expect("valid signature");
        assert_eq!(event.kind, Kind::Custom(38172));
        assert_eq!(event.pubkey, keys.public_key());
        let tags: Vec<_> = event.tags.iter().map(|tag| tag.as_slice()).collect();
        assert_eq!(tags[0], ["d", &info.pubkey.expect("pubkey").to_string()]);
        assert_eq!(tags[1], ["u", "https://mint.example/cashu"]);
        assert_eq!(tags[2], ["nuts", "1,2,3,6"]);
        assert_eq!(tags[3], ["n", "regtest"]);
        assert_eq!(
            Metadata::from_json(&event.content).expect("metadata").name,
            info.name
        );
    }

    #[test]
    fn advertises_only_enabled_capabilities() {
        let mut info = info();
        info.nuts.nut04 = nut04::Settings::new(
            vec![nut04::MintMethodSettings {
                method: PaymentMethod::BOLT11,
                unit: CurrencyUnit::Sat,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            }],
            false,
        );
        info.nuts.nut05 = nut05::Settings::new(
            vec![nut05::MeltMethodSettings {
                method: PaymentMethod::BOLT11,
                unit: CurrencyUnit::Sat,
                method_name: None,
                min_amount: None,
                max_amount: None,
                options: None,
            }],
            true,
        );
        info.nuts.nut07.supported = true;
        info.nuts.nut12.supported = true;
        let event = mint_announcement("https://mint.example", &info, "mainnet")
            .expect("announcement")
            .finalize(&Keys::generate())
            .expect("signed");
        assert!(event
            .tags
            .iter()
            .any(|tag| tag.as_slice() == ["nuts", "1,2,3,4,6,7,12"]));
    }

    #[test]
    fn rejects_missing_identity_and_invalid_configuration() {
        assert!(matches!(
            mint_announcement("https://mint.example", &MintInfo::new(), "mainnet"),
            Err(Error::MissingPublicKey)
        ));
        for url in [
            "not a URL",
            "ftp://mint.example",
            "https://user:pass@mint.example",
            "https://mint.example/#fragment",
            "https://mint.example/?key=secret",
        ] {
            assert!(matches!(
                mint_announcement(url, &info(), "mainnet"),
                Err(Error::InvalidUrl)
            ));
        }
        assert!(matches!(
            mint_announcement("https://mint.example", &info(), "bitcoin"),
            Err(Error::InvalidNetwork)
        ));
        for network in ["mainnet", "testnet", "signet", "regtest"] {
            assert!(mint_announcement("https://mint.example", &info(), network).is_ok());
        }
    }
}
