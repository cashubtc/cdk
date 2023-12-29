use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::nut00::wallet::Token as TokenSdk;
use cashu::nuts::CurrencyUnit as CurrencyUnitSdk;
use cashu::url::UncheckedUrl;

use crate::error::Result;
use crate::{MintProofs, Proof};

pub enum CurrencyUnit {
    Sat(),
    Usd(),
    Custom { unit: String },
}

impl From<&CurrencyUnit> for CurrencyUnitSdk {
    fn from(unit: &CurrencyUnit) -> CurrencyUnitSdk {
        match unit {
            CurrencyUnit::Sat() => CurrencyUnitSdk::Sat,
            CurrencyUnit::Usd() => CurrencyUnitSdk::Usd,
            CurrencyUnit::Custom { unit } => CurrencyUnitSdk::Custom(unit.clone()),
        }
    }
}

impl From<CurrencyUnit> for CurrencyUnitSdk {
    fn from(unit: CurrencyUnit) -> CurrencyUnitSdk {
        match unit {
            CurrencyUnit::Sat() => CurrencyUnitSdk::Sat,
            CurrencyUnit::Usd() => CurrencyUnitSdk::Usd,
            CurrencyUnit::Custom { unit } => CurrencyUnitSdk::Custom(unit.clone()),
        }
    }
}

impl From<CurrencyUnitSdk> for CurrencyUnit {
    fn from(unit: CurrencyUnitSdk) -> CurrencyUnit {
        match unit {
            CurrencyUnitSdk::Sat => CurrencyUnit::Sat(),
            CurrencyUnitSdk::Usd => CurrencyUnit::Usd(),
            CurrencyUnitSdk::Custom(unit) => CurrencyUnit::Custom { unit: unit.clone() },
        }
    }
}

pub struct Token {
    inner: TokenSdk,
}

impl Token {
    pub fn new(
        mint: String,
        proofs: Vec<Arc<Proof>>,
        memo: Option<String>,
        unit: Option<String>,
    ) -> Result<Self> {
        let mint = UncheckedUrl::from_str(&mint)?;
        let proofs = proofs.into_iter().map(|p| p.as_ref().into()).collect();

        let unit = unit.map(|u| CurrencyUnitSdk::from_str(&u).unwrap_or_default());

        Ok(Self {
            inner: TokenSdk::new(mint, proofs, memo, unit)?,
        })
    }

    pub fn token(&self) -> Vec<Arc<MintProofs>> {
        self.inner
            .token
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }

    pub fn memo(&self) -> Option<String> {
        self.inner.memo.clone()
    }

    pub fn unit(&self) -> Option<String> {
        self.inner
            .unit
            .clone()
            .map(|u| Into::<CurrencyUnitSdk>::into(u).to_string())
    }

    pub fn from_string(token: String) -> Result<Self> {
        Ok(Self {
            inner: TokenSdk::from_str(&token)?,
        })
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl From<cashu::nuts::nut00::wallet::Token> for Token {
    fn from(inner: cashu::nuts::nut00::wallet::Token) -> Token {
        Token { inner }
    }
}
