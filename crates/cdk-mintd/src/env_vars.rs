use std::env;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{anyhow, bail, Result};
use cdk::nuts::CurrencyUnit;

use crate::config::{
    Cln, Database, DatabaseEngine, FakeWallet, Info, LNbits, Ln, LnBackend, Lnd, MintInfo,
    Phoenixd, Settings, Strike,
};

pub const DATABASE_ENV_VAR: &str = "CDK_MINTD_DATABASE";
pub const ENV_URL: &str = "CDK_MINTD_URL";
pub const ENV_LISTEN_HOST: &str = "CDK_MINTD_LISTEN_HOST";
pub const ENV_LISTEN_PORT: &str = "CDK_MINTD_LISTEN_PORT";
pub const ENV_MNEMONIC: &str = "CDK_MINTD_MNEMONIC";
pub const ENV_SECONDS_QUOTE_VALID: &str = "CDK_MINTD_SECONDS_QUOTE_VALID";
pub const ENV_CACHE_SECONDS: &str = "CDK_MINTD_CACHE_SECONDS";
pub const ENV_EXTEND_CACHE_SECONDS: &str = "CDK_MINTD_EXTEND_CACHE_SECONDS";
pub const ENV_INPUT_FEE_PPK: &str = "CDK_MINTD_INPUT_FEE_PPK";
pub const ENV_ENABLE_SWAGGER: &str = "CDK_MINTD_ENABLE_SWAGGER";
// MintInfo
pub const ENV_MINT_NAME: &str = "CDK_MINTD_MINT_NAME";
pub const ENV_MINT_PUBKEY: &str = "CDK_MINTD_MINT_PUBKEY";
pub const ENV_MINT_DESCRIPTION: &str = "CDK_MINTD_MINT_DESCRIPTION";
pub const ENV_MINT_DESCRIPTION_LONG: &str = "CDK_MINTD_MINT_DESCRIPTION_LONG";
pub const ENV_MINT_ICON_URL: &str = "CDK_MINTD_MINT_ICON_URL";
pub const ENV_MINT_MOTD: &str = "CDK_MINTD_MINT_MOTD";
pub const ENV_MINT_CONTACT_NOSTR: &str = "CDK_MINTD_MINT_CONTACT_NOSTR";
pub const ENV_MINT_CONTACT_EMAIL: &str = "CDK_MINTD_MINT_CONTACT_EMAIL";
// LN
pub const ENV_LN_BACKEND: &str = "CDK_MINTD_LN_BACKEND";
pub const ENV_LN_INVOICE_DESCRIPTION: &str = "CDK_MINTD_LN_INVOICE_DESCRIPTION";
pub const ENV_LN_MIN_MINT: &str = "CDK_MINTD_LN_MIN_MINT";
pub const ENV_LN_MAX_MINT: &str = "CDK_MINTD_LN_MAX_MINT";
pub const ENV_LN_MIN_MELT: &str = "CDK_MINTD_LN_MIN_MELT";
pub const ENV_LN_MAX_MELT: &str = "CDK_MINTD_LN_MAX_MELT";
// CLN
pub const ENV_CLN_RPC_PATH: &str = "CDK_MINTD_CLN_RPC_PATH";
pub const ENV_CLN_BOLT12: &str = "CDK_MINTD_CLN_BOLT12";
pub const ENV_CLN_FEE_PERCENT: &str = "CDK_MINTD_CLN_FEE_PERCENT";
pub const ENV_CLN_RESERVE_FEE_MIN: &str = "CDK_MINTD_CLN_RESERVE_FEE_MIN";
// Strike
pub const ENV_STRIKE_API_KEY: &str = "CDK_MINTD_STRIKE_API_KEY";
pub const ENV_STRIKE_SUPPORTED_UNITS: &str = "CDK_MINTD_STRIKE_SUPPORTED_UNITS";
// LND environment variables
pub const ENV_LND_ADDRESS: &str = "CDK_MINTD_LND_ADDRESS";
pub const ENV_LND_CERT_FILE: &str = "CDK_MINTD_LND_CERT_FILE";
pub const ENV_LND_MACAROON_FILE: &str = "CDK_MINTD_LND_MACAROON_FILE";
pub const ENV_LND_FEE_PERCENT: &str = "CDK_MINTD_LND_FEE_PERCENT";
pub const ENV_LND_RESERVE_FEE_MIN: &str = "CDK_MINTD_LND_RESERVE_FEE_MIN";
// Phoenixd environment variables
pub const ENV_PHOENIXD_API_PASSWORD: &str = "CDK_MINTD_PHOENIXD_API_PASSWORD";
pub const ENV_PHOENIXD_API_URL: &str = "CDK_MINTD_PHOENIXD_API_URL";
pub const ENV_PHOENIXD_BOLT12: &str = "CDK_MINTD_PHOENIXD_BOLT12";
pub const ENV_PHOENIXD_FEE_PERCENT: &str = "CDK_MINTD_PHOENIXD_FEE_PERCENT";
pub const ENV_PHOENIXD_RESERVE_FEE_MIN: &str = "CDK_MINTD_PHOENIXD_RESERVE_FEE_MIN";
// LNBits
pub const ENV_LNBITS_ADMIN_API_KEY: &str = "CDK_MINTD_LNBITS_ADMIN_API_KEY";
pub const ENV_LNBITS_INVOICE_API_KEY: &str = "CDK_MINTD_LNBITS_INVOICE_API_KEY";
pub const ENV_LNBITS_API: &str = "CDK_MINTD_LNBITS_API";
pub const ENV_LNBITS_FEE_PERCENT: &str = "CDK_MINTD_LNBITS_FEE_PERCENT";
pub const ENV_LNBITS_RESERVE_FEE_MIN: &str = "CDK_MINTD_LNBITS_RESERVE_FEE_MIN";
// Fake Wallet
pub const ENV_FAKE_WALLET_SUPPORTED_UNITS: &str = "CDK_MINTD_FAKE_WALLET_SUPPORTED_UNITS";
pub const ENV_FAKE_WALLET_FEE_PERCENT: &str = "CDK_MINTD_FAKE_WALLET_FEE_PERCENT";
pub const ENV_FAKE_WALLET_RESERVE_FEE_MIN: &str = "CDK_MINTD_FAKE_WALLET_RESERVE_FEE_MIN";
pub const ENV_FAKE_WALLET_MIN_DELAY: &str = "CDK_MINTD_FAKE_WALLET_MIN_DELAY";
pub const ENV_FAKE_WALLET_MAX_DELAY: &str = "CDK_MINTD_FAKE_WALLET_MAX_DELAY";

impl Settings {
    pub fn from_env(&mut self) -> Result<Self> {
        if let Ok(database) = env::var(DATABASE_ENV_VAR) {
            let engine = DatabaseEngine::from_str(&database).map_err(|err| anyhow!(err))?;
            self.database = Database { engine };
        }

        self.info = self.info.clone().from_env();
        self.mint_info = self.mint_info.clone().from_env();
        self.ln = self.ln.clone().from_env();

        match self.ln.ln_backend {
            LnBackend::Cln => {
                self.cln = Some(self.cln.clone().unwrap_or_default().from_env());
            }
            LnBackend::Strike => {
                self.strike = Some(self.strike.clone().unwrap_or_default().from_env());
            }
            LnBackend::LNbits => {
                self.lnbits = Some(self.lnbits.clone().unwrap_or_default().from_env());
            }
            LnBackend::FakeWallet => {
                self.fake_wallet = Some(self.fake_wallet.clone().unwrap_or_default().from_env());
            }
            LnBackend::Phoenixd => {
                self.phoenixd = Some(self.phoenixd.clone().unwrap_or_default().from_env());
            }
            LnBackend::Lnd => {
                self.lnd = Some(self.lnd.clone().unwrap_or_default().from_env());
            }
            LnBackend::None => bail!("Ln backend must be set"),
        }

        Ok(self.clone())
    }
}

impl Info {
    pub fn from_env(mut self) -> Self {
        // Required fields
        if let Ok(url) = env::var(ENV_URL) {
            self.url = url;
        }

        if let Ok(host) = env::var(ENV_LISTEN_HOST) {
            self.listen_host = host;
        }

        if let Ok(port_str) = env::var(ENV_LISTEN_PORT) {
            if let Ok(port) = port_str.parse() {
                self.listen_port = port;
            }
        }

        if let Ok(mnemonic) = env::var(ENV_MNEMONIC) {
            self.mnemonic = mnemonic;
        }

        // Optional fields
        if let Ok(seconds_str) = env::var(ENV_SECONDS_QUOTE_VALID) {
            if let Ok(seconds) = seconds_str.parse() {
                self.seconds_quote_is_valid_for = Some(seconds);
            }
        }

        if let Ok(cache_seconds_str) = env::var(ENV_CACHE_SECONDS) {
            if let Ok(seconds) = cache_seconds_str.parse() {
                self.seconds_to_cache_requests_for = Some(seconds);
            }
        }

        if let Ok(extend_cache_str) = env::var(ENV_EXTEND_CACHE_SECONDS) {
            if let Ok(seconds) = extend_cache_str.parse() {
                self.seconds_to_extend_cache_by = Some(seconds);
            }
        }

        if let Ok(fee_str) = env::var(ENV_INPUT_FEE_PPK) {
            if let Ok(fee) = fee_str.parse() {
                self.input_fee_ppk = Some(fee);
            }
        }

        if let Ok(swagger_str) = env::var(ENV_ENABLE_SWAGGER) {
            if let Ok(enable) = swagger_str.parse() {
                self.enable_swagger_ui = Some(enable);
            }
        }

        self
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

        self
    }
}

impl Ln {
    pub fn from_env(mut self) -> Self {
        // LnBackend
        if let Ok(backend_str) = env::var(ENV_LN_BACKEND) {
            if let Ok(backend) = backend_str.parse() {
                self.ln_backend = backend;
            }
        }

        // Optional invoice description
        if let Ok(description) = env::var(ENV_LN_INVOICE_DESCRIPTION) {
            self.invoice_description = Some(description);
        }

        // Amount fields
        if let Ok(min_mint_str) = env::var(ENV_LN_MIN_MINT) {
            if let Ok(amount) = min_mint_str.parse::<u64>() {
                self.min_mint = amount.into();
            }
        }

        if let Ok(max_mint_str) = env::var(ENV_LN_MAX_MINT) {
            if let Ok(amount) = max_mint_str.parse::<u64>() {
                self.max_mint = amount.into();
            }
        }

        if let Ok(min_melt_str) = env::var(ENV_LN_MIN_MELT) {
            if let Ok(amount) = min_melt_str.parse::<u64>() {
                self.min_melt = amount.into();
            }
        }

        if let Ok(max_melt_str) = env::var(ENV_LN_MAX_MELT) {
            if let Ok(amount) = max_melt_str.parse::<u64>() {
                self.max_melt = amount.into();
            }
        }

        self
    }
}

impl Cln {
    pub fn from_env(mut self) -> Self {
        // RPC Path
        if let Ok(path) = env::var(ENV_CLN_RPC_PATH) {
            self.rpc_path = PathBuf::from(path);
        }

        // BOLT12 flag
        if let Ok(bolt12_str) = env::var(ENV_CLN_BOLT12) {
            if let Ok(bolt12) = bolt12_str.parse() {
                self.bolt12 = bolt12;
            }
        }

        // Fee percent
        if let Ok(fee_str) = env::var(ENV_CLN_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        // Reserve fee minimum
        if let Ok(reserve_fee_str) = env::var(ENV_CLN_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        self
    }
}

impl Strike {
    pub fn from_env(mut self) -> Self {
        // API Key
        if let Ok(api_key) = env::var(ENV_STRIKE_API_KEY) {
            self.api_key = api_key;
        }

        // Supported Units - expects comma-separated list
        if let Ok(units_str) = env::var(ENV_STRIKE_SUPPORTED_UNITS) {
            self.supported_units = Some(
                units_str
                    .split(',')
                    .filter_map(|s| s.trim().parse().ok())
                    .collect(),
            );
        }

        self
    }
}

impl Lnd {
    pub fn from_env(mut self) -> Self {
        if let Ok(address) = env::var(ENV_LND_ADDRESS) {
            self.address = address;
        }

        if let Ok(cert_path) = env::var(ENV_LND_CERT_FILE) {
            self.cert_file = PathBuf::from(cert_path);
        }

        if let Ok(macaroon_path) = env::var(ENV_LND_MACAROON_FILE) {
            self.macaroon_file = PathBuf::from(macaroon_path);
        }

        if let Ok(fee_str) = env::var(ENV_LND_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_LND_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        self
    }
}

impl Phoenixd {
    pub fn from_env(mut self) -> Self {
        if let Ok(password) = env::var(ENV_PHOENIXD_API_PASSWORD) {
            self.api_password = password;
        }

        if let Ok(url) = env::var(ENV_PHOENIXD_API_URL) {
            self.api_url = url;
        }

        if let Ok(bolt12_str) = env::var(ENV_PHOENIXD_BOLT12) {
            if let Ok(bolt12) = bolt12_str.parse() {
                self.bolt12 = bolt12;
            }
        }

        if let Ok(fee_str) = env::var(ENV_PHOENIXD_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_PHOENIXD_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        self
    }
}

impl LNbits {
    pub fn from_env(mut self) -> Self {
        if let Ok(admin_key) = env::var(ENV_LNBITS_ADMIN_API_KEY) {
            self.admin_api_key = admin_key;
        }

        if let Ok(invoice_key) = env::var(ENV_LNBITS_INVOICE_API_KEY) {
            self.invoice_api_key = invoice_key;
        }

        if let Ok(api) = env::var(ENV_LNBITS_API) {
            self.lnbits_api = api;
        }

        if let Ok(fee_str) = env::var(ENV_LNBITS_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_LNBITS_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        self
    }
}

impl FakeWallet {
    pub fn from_env(mut self) -> Self {
        // Supported Units - expects comma-separated list
        if let Ok(units_str) = env::var(ENV_FAKE_WALLET_SUPPORTED_UNITS) {
            if let Ok(units) = units_str
                .split(',')
                .map(|s| s.trim().parse())
                .collect::<Result<Vec<CurrencyUnit>, _>>()
            {
                self.supported_units = units;
            }
        }

        if let Ok(fee_str) = env::var(ENV_FAKE_WALLET_FEE_PERCENT) {
            if let Ok(fee) = fee_str.parse() {
                self.fee_percent = fee;
            }
        }

        if let Ok(reserve_fee_str) = env::var(ENV_FAKE_WALLET_RESERVE_FEE_MIN) {
            if let Ok(reserve_fee) = reserve_fee_str.parse::<u64>() {
                self.reserve_fee_min = reserve_fee.into();
            }
        }

        if let Ok(min_delay_str) = env::var(ENV_FAKE_WALLET_MIN_DELAY) {
            if let Ok(min_delay) = min_delay_str.parse() {
                self.min_delay_time = min_delay;
            }
        }

        if let Ok(max_delay_str) = env::var(ENV_FAKE_WALLET_MAX_DELAY) {
            if let Ok(max_delay) = max_delay_str.parse() {
                self.max_delay_time = max_delay;
            }
        }

        self
    }
}
