#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use cashu::nuts::{KeySetVersion, PublicKey, SecretKey};

    #[test]
    fn ordinary_key_api_remains_source_compatible() {
        let secret = SecretKey::from_slice(&[1; 32]).unwrap();
        let public = secret.public_key();
        let bytes: [u8; 33] = public.to_bytes();
        let uncompressed: [u8; 65] = public.to_uncompressed_bytes();
        let _: &[u8] = secret.as_secret_bytes();
        let _ = secret.as_scalar();
        let _ = secret.clone().to_scalar();
        let _ = public.x_only_public_key().serialize();
        let _: [u8; 32] = cashu::dhke::hash_e([public]);
        let _ = public.deref().serialize();
        let _ = secret.deref().secret_bytes();
        assert_eq!(PublicKey::from_slice(&bytes).unwrap(), public);
        assert_eq!(PublicKey::from_slice(&uncompressed).unwrap(), public);
        assert!(PublicKey::from_slice(&[0; 48]).is_err());
        assert!(PublicKey::from_slice(&[0; 96]).is_err());
        assert_eq!(serde_json::to_value(public).unwrap(), public.to_hex());
        assert_eq!(
            serde_json::to_value(secret.clone()).unwrap(),
            secret.to_secret_hex()
        );
    }

    #[test]
    fn ordinary_keyset_api_and_versions_remain_source_compatible() {
        fn exhaustive(version: KeySetVersion) -> u8 {
            match version {
                KeySetVersion::Version00 => 0,
                KeySetVersion::Version01 => 1,
            }
        }
        assert_eq!(exhaustive(KeySetVersion::Version01), 1);
        assert!(KeySetVersion::from_byte(&2).is_err());
        let _ = cdk::mint::KeysetRotation {
            unit: cashu::CurrencyUnit::Sat,
            amounts: vec![1, 2, 4],
            input_fee_ppk: 0,
            use_keyset_v2: true,
            final_expiry: None,
        };
    }
}
