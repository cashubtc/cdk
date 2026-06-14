mod convert;

tonic::include_proto!("signatory");

pub(crate) const ENV_SIGNATORY_ALLOW_INSECURE: &str = "CDK_SIGNATORY_ALLOW_INSECURE";

pub(crate) fn allow_insecure_signatory_rpc() -> std::io::Result<bool> {
    match std::env::var(ENV_SIGNATORY_ALLOW_INSECURE) {
        Ok(value) => value.parse::<bool>().map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{ENV_SIGNATORY_ALLOW_INSECURE} must be true or false: {err}"),
            )
        }),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(err) => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, err)),
    }
}

pub mod client;
pub mod server;
