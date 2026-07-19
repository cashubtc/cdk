//! Management RPC client connection utilities.

use std::path::{Path, PathBuf};

use cdk_common::grpc::{VersionInterceptor, VERSION_HEADER};
use thiserror::Error;
use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity};

use crate::{cdk_mint_client::CdkMintClient, InterceptedCdkMintClient};

/// Management RPC client connection error.
#[derive(Debug, Error)]
pub enum ClientError {
    /// TLS credentials were supplied for a plaintext endpoint.
    #[error(
        "Management RPC address `{address}` must use `https://` when TLS credentials are configured"
    )]
    TlsRequiresHttps {
        /// Address rejected by the client.
        address: String,
    },
    /// No TLS credentials were supplied for a secure endpoint.
    #[error(
        "Management RPC address `{address}` must use `http://` when TLS credentials are not configured"
    )]
    PlaintextRequiresHttp {
        /// Address rejected by the client.
        address: String,
    },
    /// A TLS credential could not be read.
    #[error("Could not read management RPC TLS file {}: {source}", path.display())]
    ReadTlsFile {
        /// Path to the TLS credential.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The RPC endpoint or transport failed.
    #[error(transparent)]
    Transport(#[from] tonic::transport::Error),
}

/// Connects to the mint management RPC server.
///
/// When `tls_dir` is set, the directory must contain `ca.pem`, `client.pem`,
/// and `client.key`, and `addr` must use `https://`. When it is not set, `addr`
/// must use `http://` and the connection uses plaintext. Every request includes
/// the mint management protocol-version header.
pub async fn connect_client(
    addr: &str,
    tls_dir: Option<&Path>,
) -> Result<InterceptedCdkMintClient, ClientError> {
    validate_security_scheme(addr, tls_dir.is_some())?;
    let endpoint = Endpoint::from_shared(addr.to_owned())?;

    let channel = match tls_dir {
        Some(tls_dir) => {
            #[cfg(not(target_arch = "wasm32"))]
            if rustls::crypto::CryptoProvider::get_default().is_none() {
                let _ = rustls::crypto::ring::default_provider().install_default();
            }

            let ca_path = tls_dir.join("ca.pem");
            let client_cert_path = tls_dir.join("client.pem");
            let client_key_path = tls_dir.join("client.key");
            let ca = read_tls_file(&ca_path)?;
            let client_cert = read_tls_file(&client_cert_path)?;
            let client_key = read_tls_file(&client_key_path)?;

            let tls = ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(ca))
                .identity(Identity::from_pem(client_cert, client_key));

            endpoint.tls_config(tls)?.connect().await?
        }
        None => endpoint.connect().await?,
    };

    let interceptor =
        VersionInterceptor::new(VERSION_HEADER, cdk_common::MINT_RPC_PROTOCOL_VERSION);

    Ok(CdkMintClient::with_interceptor(channel, interceptor))
}

fn validate_security_scheme(addr: &str, tls_enabled: bool) -> Result<(), ClientError> {
    let scheme = addr
        .split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or_default();

    match tls_enabled {
        true if !scheme.eq_ignore_ascii_case("https") => Err(ClientError::TlsRequiresHttps {
            address: addr.to_owned(),
        }),
        false if !scheme.eq_ignore_ascii_case("http") => Err(ClientError::PlaintextRequiresHttp {
            address: addr.to_owned(),
        }),
        _ => Ok(()),
    }
}

fn read_tls_file(path: &Path) -> Result<Vec<u8>, ClientError> {
    std::fs::read(path).map_err(|source| ClientError::ReadTlsFile {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_plaintext_address_when_tls_is_configured() {
        let address = "http://127.0.0.1:8086";
        let error = validate_security_scheme(address, true).expect_err("scheme must be rejected");

        assert!(matches!(
            error,
            ClientError::TlsRequiresHttps { address: rejected } if rejected == address
        ));
    }

    #[test]
    fn rejects_secure_address_without_tls_credentials() {
        let address = "https://127.0.0.1:8086";
        let error = validate_security_scheme(address, false).expect_err("scheme must be rejected");

        assert!(matches!(
            error,
            ClientError::PlaintextRequiresHttp { address: rejected } if rejected == address
        ));
    }

    #[test]
    fn accepts_matching_security_schemes() {
        validate_security_scheme("https://127.0.0.1:8086", true)
            .expect("HTTPS must be accepted with TLS credentials");
        validate_security_scheme("http://127.0.0.1:8086", false)
            .expect("HTTP must be accepted without TLS credentials");
    }
}
