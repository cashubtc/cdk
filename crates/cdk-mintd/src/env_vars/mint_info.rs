//! MintInfo environment variables

use std::env;

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
        if env::var_os(ENV_MINT_PUBKEY).is_some() {
            tracing::warn!(
                "Ignoring legacy {ENV_MINT_PUBKEY}; the mint public key is derived from the signer"
            );
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

        self
    }
}
