#[cfg(test)]
mod tests {
    use cashu::nuts::{KeySetVersion, PublicKey, SecretKey};

    /// Ordinary builds use the same key types as federation builds: no
    /// feature changes a public type's shape.
    #[test]
    fn key_types_do_not_depend_on_federation() {
        let secret = SecretKey::from_slice(&[1; 32]).unwrap();
        let public = secret.public_key();
        let bytes = public.to_bytes();
        assert_eq!(bytes.len(), 33);
        assert_eq!(PublicKey::from_slice(&bytes).unwrap(), public);
        assert_eq!(serde_json::to_value(public).unwrap(), public.to_hex());
        assert_eq!(
            serde_json::to_value(secret.clone()).unwrap(),
            secret.to_secret_hex()
        );

        let bls_public = SecretKey::generate_bls().public_key();
        assert_eq!(
            PublicKey::from_slice(&bls_public.to_bytes()).unwrap(),
            bls_public
        );
    }

    /// BLS v3 keysets are part of the ordinary protocol API.
    #[test]
    fn v3_keysets_are_part_of_the_ordinary_api() {
        assert!(KeySetVersion::Version02.is_bls());
        assert_eq!(
            KeySetVersion::from_byte(&2).unwrap(),
            KeySetVersion::Version02
        );
        let _ = cdk::mint::KeysetRotation {
            unit: cashu::CurrencyUnit::Sat,
            amounts: vec![1, 2, 4],
            input_fee_ppk: 0,
            use_keyset_v2: true,
            final_expiry: None,
        };
    }
}
