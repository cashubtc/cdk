//! GRPC Client

use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use cdk_common::util::hex;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::rt::TokioExecutor;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, Error as TLSError, SignatureScheme};
use tokio::fs;
use tonic::body::Body;
use tonic::codegen::InterceptedService;
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::{Request, Status};

use crate::{lnrpc, routerrpc, Error};

/// Custom certificate verifier for LND's self-signed certificates
#[derive(Debug)]
pub(crate) struct LndCertVerifier {
    certs: Vec<Vec<u8>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl LndCertVerifier {
    pub(crate) async fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let provider = default_provider();

        let contents = fs::read(path).await.map_err(|_| Error::ReadFile)?;
        let mut reader = std::io::Cursor::new(contents);

        // Parse PEM certificates
        let certs: Vec<CertificateDer<'static>> =
            rustls_pemfile::certs(&mut reader).flatten().collect();

        Ok(LndCertVerifier {
            certs: certs.into_iter().map(|c| c.to_vec()).collect(),
            provider: Arc::new(provider),
        })
    }
}

impl ServerCertVerifier for LndCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TLSError> {
        let mut certs = intermediates
            .iter()
            .map(|c| c.as_ref().to_vec())
            .collect::<Vec<Vec<u8>>>();
        certs.push(end_entity.as_ref().to_vec());
        certs.sort();

        let mut our_certs = self.certs.clone();
        our_certs.sort();

        if self.certs.len() != certs.len() {
            return Err(TLSError::General(format!(
                "Mismatched number of certificates (Expected: {}, Presented: {})",
                self.certs.len(),
                certs.len()
            )));
        }
        for (c, p) in our_certs.iter().zip(certs.iter()) {
            if p != c {
                return Err(TLSError::General(
                    "Server certificates do not match ours".to_string(),
                ));
            }
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TLSError> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
        .map(|_| HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TLSError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
        .map(|_| HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub type RouterClient = routerrpc::router_client::RouterClient<
    InterceptedService<
        HyperClient<hyper_rustls::HttpsConnector<HttpConnector>, Body>,
        MacaroonInterceptor,
    >,
>;

/// The client returned by `connect` function
#[derive(Clone)]
pub struct Client {
    lightning: lnrpc::lightning_client::LightningClient<
        InterceptedService<
            HyperClient<hyper_rustls::HttpsConnector<HttpConnector>, Body>,
            MacaroonInterceptor,
        >,
    >,
    router: RouterClient,
}

/// Supplies requests with macaroon
#[derive(Clone)]
pub struct MacaroonInterceptor {
    macaroon: String,
}

impl Interceptor for MacaroonInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert(
            "macaroon",
            MetadataValue::from_str(&self.macaroon)
                .map_err(|e| Status::internal(format!("Invalid macaroon: {e}")))?,
        );
        Ok(request)
    }
}

async fn load_macaroon(path: impl AsRef<Path>) -> Result<String, Error> {
    let macaroon = fs::read(path).await.map_err(|_| Error::ReadFile)?;
    Ok(hex::encode(macaroon))
}

pub async fn connect<P: AsRef<Path>>(
    address: &str,
    cert_path: P,
    macaroon_path: P,
) -> Result<Client, Error> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(LndCertVerifier::load(cert_path).await?))
        .with_no_client_auth();

    // Create HTTPS connector
    let https = HttpsConnectorBuilder::new()
        .with_tls_config(config)
        .https_only()
        .enable_http2()
        .build();

    // Create hyper client
    let client = HyperClient::builder(TokioExecutor::new())
        .http2_only(true)
        .build(https);

    // Load macaroon
    let macaroon = load_macaroon(macaroon_path).await?;

    // Create service with macaroon interceptor
    let service = InterceptedService::new(client, MacaroonInterceptor { macaroon });

    // Create URI for the service
    let address = address
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    let uri = http::Uri::from_str(&format!("https://{address}"))
        .map_err(|e| Error::InvalidConfig(format!("Invalid URI: {e}")))?;

    // Create LND client
    let lightning =
        lnrpc::lightning_client::LightningClient::with_origin(service.clone(), uri.clone());
    let router = RouterClient::with_origin(service, uri);

    Ok(Client { lightning, router })
}

impl Client {
    pub fn lightning(
        &mut self,
    ) -> &mut lnrpc::lightning_client::LightningClient<
        InterceptedService<
            HyperClient<hyper_rustls::HttpsConnector<HttpConnector>, Body>,
            MacaroonInterceptor,
        >,
    > {
        &mut self.lightning
    }

    pub fn router(&mut self) -> &mut RouterClient {
        &mut self.router
    }
}
