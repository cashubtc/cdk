use core::fmt;

use cdk::mint_url::MintUrl;
use cdk::wallet::{AuthWallet, BaseHttpClient};
use cdk_http_client::Async;
use uuid::Uuid;

use crate::error::Result;
use crate::transport::{EnclaviaTransport, MintTarget};
use crate::Pcrs;

/// A CDK mint client whose mint requests use an attested Enclavia channel.
pub type EnclaviaClient = BaseHttpClient<EnclaviaTransport>;

#[derive(Debug, Clone)]
struct TrustUpgrades {
    backend_url: String,
    enclave_id: Uuid,
}

/// Builder for an [`EnclaviaClient`].
pub struct EnclaviaClientBuilder {
    mint_url: MintUrl,
    enclave_url: String,
    pcrs: Pcrs,
    debug_mode: bool,
    headers: Vec<(String, String)>,
    trust_upgrades: Option<TrustUpgrades>,
    auth_wallet: Option<AuthWallet>,
    fallback: Async,
}

impl fmt::Debug for EnclaviaClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnclaviaClientBuilder")
            .field("mint_url", &self.mint_url)
            .field("enclave_url", &self.enclave_url)
            .field("pcrs", &self.pcrs)
            .field("debug_mode", &self.debug_mode)
            .field("header_count", &self.headers.len())
            .field("trust_upgrades", &self.trust_upgrades)
            .field("has_auth_wallet", &self.auth_wallet.is_some())
            .finish_non_exhaustive()
    }
}

impl EnclaviaClientBuilder {
    /// Create a builder with the mint identity, Enclavia endpoint, and pinned PCRs.
    pub fn new(mint_url: MintUrl, enclave_url: impl Into<String>, pcrs: Pcrs) -> Self {
        Self {
            mint_url,
            enclave_url: enclave_url.into(),
            pcrs,
            debug_mode: false,
            headers: Vec::new(),
            trust_upgrades: None,
            auth_wallet: None,
            fallback: Async::default(),
        }
    }

    /// Enable Enclavia debug-mode attestation verification.
    ///
    /// This is intended only for local QEMU development and must not be used
    /// for a production mint.
    pub fn debug_mode(mut self, enabled: bool) -> Self {
        self.debug_mode = enabled;
        self
    }

    /// Trust hardware-attested upgrades descending from the pinned PCRs.
    pub fn trust_upgrades(mut self, backend_url: impl Into<String>, enclave_id: Uuid) -> Self {
        self.trust_upgrades = Some(TrustUpgrades {
            backend_url: backend_url.into(),
            enclave_id,
        });
        self
    }

    /// Add a header to the initial Enclavia WebSocket upgrade request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Set the auth wallet used by CDK for protected mint endpoints.
    pub fn auth_wallet(mut self, auth_wallet: AuthWallet) -> Self {
        self.auth_wallet = Some(auth_wallet);
        self
    }

    /// Replace the normal transport used for non-mint HTTP requests.
    ///
    /// This fallback is used for external services such as LNURL callbacks
    /// and OIDC issuers. It is never used for the configured mint origin.
    pub fn fallback_transport(mut self, fallback: Async) -> Self {
        self.fallback = fallback;
        self
    }

    /// Connect, verify the enclave attestation, and construct the CDK client.
    pub async fn build(self) -> Result<EnclaviaClient> {
        let target = MintTarget::new(&self.mint_url)?;

        let mut builder = enclavia::Client::builder(&self.enclave_url)
            .pcrs(self.pcrs)
            .debug_mode(self.debug_mode);

        for (name, value) in self.headers {
            builder = builder.header(name, value);
        }

        if let Some(trust_upgrades) = self.trust_upgrades {
            builder = builder.trust_upgrades(trust_upgrades.backend_url, trust_upgrades.enclave_id);
        }

        let enclavia_client = builder.build().await?;
        let transport = EnclaviaTransport::from_parts(target, enclavia_client, self.fallback);

        Ok(BaseHttpClient::with_transport(
            self.mint_url,
            transport,
            self.auth_wallet,
        ))
    }
}

/// Connect to an enclave using strict PCR pinning and construct a CDK client.
pub async fn connect(
    mint_url: MintUrl,
    enclave_url: impl Into<String>,
    pcrs: Pcrs,
) -> Result<EnclaviaClient> {
    EnclaviaClientBuilder::new(mint_url, enclave_url, pcrs)
        .build()
        .await
}
