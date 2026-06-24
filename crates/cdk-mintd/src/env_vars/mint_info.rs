//! MintInfo environment variables

use std::env;

#[cfg(feature = "conditional-tokens")]
use crate::config::CtfRegistrationFeeConfig;
use crate::config::MintInfo;

// MintInfo environment variables
pub const ENV_MINT_NAME: &str = "CDK_MINTD_MINT_NAME";
pub const ENV_MINT_PUBKEY: &str = "CDK_MINTD_MINT_PUBKEY";
pub const ENV_MINT_DESCRIPTION: &str = "CDK_MINTD_MINT_DESCRIPTION";
pub const ENV_MINT_DESCRIPTION_LONG: &str = "CDK_MINTD_MINT_DESCRIPTION_LONG";
pub const ENV_MINT_ICON_URL: &str = "CDK_MINTD_MINT_ICON_URL";
pub const ENV_MINT_MOTD: &str = "CDK_MINTD_MINT_MOTD";
pub const ENV_MINT_CONTACT_NOSTR: &str = "CDK_MINTD_MINT_CONTACT_NOSTR";
pub const ENV_MINT_CONTACT_EMAIL: &str = "CDK_MINTD_MINT_CONTACT_EMAIL";
pub const ENV_MINT_TOS_URL: &str = "CDK_MINTD_MINT_TOS_URL";
#[cfg(feature = "conditional-tokens")]
pub const ENV_MINT_CTF_DEFAULT_KEYSET_CREATION: &str = "CDK_MINTD_CTF_DEFAULT_KEYSET_CREATION";
#[cfg(feature = "conditional-tokens")]
pub const ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE: &str =
    "CDK_MINTD_CTF_REGISTRATION_FEE_MSAT_BASE";
#[cfg(feature = "conditional-tokens")]
pub const ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET: &str =
    "CDK_MINTD_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET";
#[cfg(feature = "conditional-tokens")]
pub const ENV_MINT_CTF_REGISTRATION_FEE_USD_BASE: &str = "CDK_MINTD_CTF_REGISTRATION_FEE_USD_BASE";
#[cfg(feature = "conditional-tokens")]
pub const ENV_MINT_CTF_REGISTRATION_FEE_USD_PER_KEYSET: &str =
    "CDK_MINTD_CTF_REGISTRATION_FEE_USD_PER_KEYSET";

#[cfg(feature = "conditional-tokens")]
fn registration_fee_from_env(
    unit: &str,
    base_env: &str,
    per_keyset_env: &str,
) -> Option<CtfRegistrationFeeConfig> {
    let base = env::var(base_env).ok().and_then(|value| value.parse().ok());
    let per_keyset = env::var(per_keyset_env)
        .ok()
        .and_then(|value| value.parse().ok());

    match (base, per_keyset) {
        (Some(base), Some(per_keyset)) => Some(CtfRegistrationFeeConfig {
            unit: unit.to_string(),
            base,
            per_keyset,
        }),
        (Some(_), None) | (None, Some(_)) => panic!(
            "CTF registration fee for unit '{unit}': both {base_env} and {per_keyset_env} must be set"
        ),
        (None, None) => None,
    }
}

impl MintInfo {
    pub fn from_env(mut self) -> Self {
        // Required fields
        if let Ok(name) = env::var(ENV_MINT_NAME) {
            self.name = name;
        }

        if let Ok(description) = env::var(ENV_MINT_DESCRIPTION) {
            self.description = description;
        }

        // Optional fields
        if let Ok(pubkey_str) = env::var(ENV_MINT_PUBKEY) {
            // Assuming PublicKey has a from_str implementation
            if let Ok(pubkey) = pubkey_str.parse() {
                self.pubkey = Some(pubkey);
            }
        }

        if let Ok(desc_long) = env::var(ENV_MINT_DESCRIPTION_LONG) {
            self.description_long = Some(desc_long);
        }

        if let Ok(icon_url) = env::var(ENV_MINT_ICON_URL) {
            self.icon_url = Some(icon_url);
        }

        if let Ok(motd) = env::var(ENV_MINT_MOTD) {
            self.motd = Some(motd);
        }

        if let Ok(nostr_key) = env::var(ENV_MINT_CONTACT_NOSTR) {
            self.contact_nostr_public_key = Some(nostr_key);
        }

        if let Ok(email) = env::var(ENV_MINT_CONTACT_EMAIL) {
            self.contact_email = Some(email);
        }

        if let Ok(tos_url) = env::var(ENV_MINT_TOS_URL) {
            self.tos_url = Some(tos_url);
        }

        #[cfg(feature = "conditional-tokens")]
        if let Ok(policy) = env::var(ENV_MINT_CTF_DEFAULT_KEYSET_CREATION) {
            self.ctf_default_keyset_creation = Some(policy);
        }
        #[cfg(feature = "conditional-tokens")]
        {
            let mut fees = Vec::new();
            if let Some(fee) = registration_fee_from_env(
                "msat",
                ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE,
                ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET,
            ) {
                fees.push(fee);
            }
            if let Some(fee) = registration_fee_from_env(
                "usd",
                ENV_MINT_CTF_REGISTRATION_FEE_USD_BASE,
                ENV_MINT_CTF_REGISTRATION_FEE_USD_PER_KEYSET,
            ) {
                fees.push(fee);
            }
            if !fees.is_empty() {
                self.ctf_registration_fees = Some(fees);
            }
        }

        self
    }
}

#[cfg(test)]
#[cfg(feature = "conditional-tokens")]
mod tests {
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .expect("environment lock should not be poisoned")
    }

    fn clear_registration_fee_env() {
        env::remove_var(ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE);
        env::remove_var(ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET);
    }

    #[test]
    fn registration_fee_from_env_requires_base_when_per_keyset_is_set() {
        let _guard = env_lock();
        clear_registration_fee_env();
        env::set_var(ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET, "10000");

        let result = std::panic::catch_unwind(|| {
            registration_fee_from_env(
                "msat",
                ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE,
                ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET,
            )
        });

        clear_registration_fee_env();
        assert!(result.is_err());
    }

    #[test]
    fn registration_fee_from_env_requires_per_keyset_when_base_is_set() {
        let _guard = env_lock();
        clear_registration_fee_env();
        env::set_var(ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE, "10000");

        let result = std::panic::catch_unwind(|| {
            registration_fee_from_env(
                "msat",
                ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE,
                ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET,
            )
        });

        clear_registration_fee_env();
        assert!(result.is_err());
    }

    #[test]
    fn registration_fee_from_env_returns_fee_when_both_fields_are_set() {
        let _guard = env_lock();
        clear_registration_fee_env();
        env::set_var(ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE, "10000");
        env::set_var(ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET, "20000");

        let fee = registration_fee_from_env(
            "msat",
            ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE,
            ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET,
        )
        .expect("complete fee config should be present");

        clear_registration_fee_env();
        assert_eq!(fee.unit, "msat");
        assert_eq!(fee.base, 10000);
        assert_eq!(fee.per_keyset, 20000);
    }

    #[test]
    fn registration_fee_from_env_omits_unit_when_neither_field_is_set() {
        let _guard = env_lock();
        clear_registration_fee_env();

        let fee = registration_fee_from_env(
            "msat",
            ENV_MINT_CTF_REGISTRATION_FEE_MSAT_BASE,
            ENV_MINT_CTF_REGISTRATION_FEE_MSAT_PER_KEYSET,
        );

        assert!(fee.is_none());
    }
}
