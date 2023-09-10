use std::fmt;

use cashu::lightning_invoice::ParseOrSemanticError;

pub type Result<T, E = CashuError> = std::result::Result<T, E>;

#[derive(Debug)]
pub enum CashuError {
    Generic { err: String },
}

impl fmt::Display for CashuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Generic { err } => write!(f, "{err}"),
        }
    }
}

impl From<cashu::error::Error> for CashuError {
    fn from(err: cashu::error::Error) -> Self {
        Self::Generic {
            err: err.to_string(),
        }
    }
}

impl From<url::ParseError> for CashuError {
    fn from(err: url::ParseError) -> Self {
        Self::Generic {
            err: err.to_string(),
        }
    }
}

impl From<cashu::error::wallet::Error> for CashuError {
    fn from(err: cashu::error::wallet::Error) -> Self {
        Self::Generic {
            err: err.to_string(),
        }
    }
}

impl From<ParseOrSemanticError> for CashuError {
    fn from(err: ParseOrSemanticError) -> Self {
        Self::Generic {
            err: err.to_string(),
        }
    }
}

impl From<cashu::nuts::nut02::Error> for CashuError {
    fn from(err: cashu::nuts::nut02::Error) -> Self {
        Self::Generic {
            err: err.to_string(),
        }
    }
}

impl From<cashu::url::Error> for CashuError {
    fn from(err: cashu::url::Error) -> Self {
        Self::Generic {
            err: err.to_string(),
        }
    }
}
