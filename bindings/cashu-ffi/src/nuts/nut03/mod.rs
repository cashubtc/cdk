use std::str::FromStr;

use cashu::nuts::nut03::RequestMintResponse as RequestMintResponseSdk;
use cashu::Bolt11Invoice;

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

impl From<cashu::nuts::nut03::RequestMintResponse> for RequestMintResponse {
    fn from(mint_response: cashu::nuts::nut03::RequestMintResponse) -> RequestMintResponse {
        RequestMintResponse {
            inner: mint_response,
        }
    }
}
