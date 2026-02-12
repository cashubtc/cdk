//! gRPC version checking utilities

use tonic::{Request, Status};

/// Header name for protocol version
pub const VERSION_HEADER: &str = "x-cdk-protocol-version";

/// Creates a client-side interceptor that injects a specific protocol version into outgoing requests
///
/// # Panics
/// Panics if the version string is not a valid gRPC metadata ASCII value
pub fn create_version_inject_interceptor(
    version: &'static str,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone {
    move |mut request: Request<()>| {
        request.metadata_mut().insert(
            VERSION_HEADER,
            version.parse().expect("Invalid protocol version"),
        );
        Ok(request)
    }
}

/// Creates a server-side interceptor that validates a specific protocol version on incoming requests
pub fn create_version_check_interceptor(
    expected_version: &'static str,
) -> impl Fn(Request<()>) -> Result<Request<()>, Status> + Clone {
    move |request: Request<()>| match request.metadata().get(VERSION_HEADER) {
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
