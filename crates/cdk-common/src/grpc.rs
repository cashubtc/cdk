//! gRPC version checking utilities

use tonic::metadata::AsciiMetadataValue;
use tonic::service::Interceptor;
use tonic::{Request, Status};

/// Header name for protocol version
pub const VERSION_HEADER: &str = "x-cdk-protocol-version";
/// Header for version of the signatory protofile
pub const VERSION_SIGNATORY_HEADER: &str = "x-signatory-schema-version";

/// A client-side interceptor that injects a protocol version header into every
/// outgoing gRPC request.
///
/// # Panics
/// [`VersionInterceptor::new`] panics if the version string is not a valid gRPC
/// metadata ASCII value.
#[derive(Debug, Clone)]
pub struct VersionInterceptor {
    header: &'static str,
    value: AsciiMetadataValue,
}

impl VersionInterceptor {
    /// Create a new `VersionInterceptor`.
    ///
    /// # Panics
    /// Panics if `version` is not a valid gRPC metadata ASCII value.
    pub fn new(header: &'static str, version: impl AsRef<str>) -> Self {
        Self {
            header,
            value: version.as_ref().parse().expect("Invalid protocol version"),
        }
    }
}

impl Interceptor for VersionInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request
            .metadata_mut()
            .insert(self.header, self.value.clone());
        Ok(request)
    }
}

/// Creates a server-side interceptor that validates a specific protocol version on incoming requests
pub fn create_version_check_interceptor(
    header: &'static str,
    expected_version: &'static str,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone {
    move |request: Request<()>| match request.metadata().get(header) {
        Some(version) => {
            let version = version
                .to_str()
                .map_err(|_| Status::invalid_argument("Invalid protocol version header"))?;
            if version != expected_version {
                return Err(Status::failed_precondition(format!(
                    "Protocol version mismatch: server={}, client={}",
                    expected_version, version
                )));
            }
            Ok(request)
        }
        None => Err(Status::failed_precondition(
            "Missing x-cdk-protocol-version header",
        )),
    }
}
