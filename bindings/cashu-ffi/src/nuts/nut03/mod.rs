use std::str::FromStr;

use cashu::{nuts::nut03::RequestMintResponse as RequestMintResponseSdk, Bolt11Invoice};

use crate::error::Result;

pub struct RequestMintResponse {
    inner: RequestMintResponseSdk,
}

impl RequestMintResponse {
    pub fn new(invoice: String, hash: String) -> Result<Self> {
        let pr = Bolt11Invoice::from_str(&invoice)?;

        Ok(Self {
            inner: RequestMintResponseSdk { pr, hash },
        })
    }

    pub fn invoice(&self) -> String {
        self.inner.pr.to_string()
    }

    pub fn hash(&self) -> String {
        self.inner.hash.to_string()
    }
}
