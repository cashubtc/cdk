use std::fmt;

pub type Result<T, E = CashuSdkError> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum CashuSdkError {
    Generic { err: String },
}

impl fmt::Display for CashuSdkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generic { err } => write!(f, "{err}"),
        }
    }
}

impl From<cashu_sdk::client::Error> for CashuSdkError {
    fn from(err: cashu_sdk::client::Error) -> CashuSdkError {
        Self::Generic {
            err: err.to_string(),
        }
    }
}

impl From<cashu_sdk::wallet::Error> for CashuSdkError {
    fn from(err: cashu_sdk::wallet::Error) -> CashuSdkError {
        Self::Generic {
            err: err.to_string(),
        }
    }
}

impl From<cashu_sdk::mint::Error> for CashuSdkError {
    fn from(err: cashu_sdk::mint::Error) -> CashuSdkError {
        Self::Generic {
            err: err.to_string(),
        }
    }
}
