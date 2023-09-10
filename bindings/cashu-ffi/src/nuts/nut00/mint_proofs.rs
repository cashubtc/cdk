use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use cashu::nuts::nut00::MintProofs as MintProofsSdk;
use cashu::url::UncheckedUrl;

use crate::error::Result;
use crate::Proof;

pub struct MintProofs {
    inner: MintProofsSdk,
}

impl Deref for MintProofs {
    type Target = MintProofsSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl MintProofs {
    pub fn new(mint: String, proofs: Vec<Arc<Proof>>) -> Result<Self> {
        let mint = UncheckedUrl::from_str(&mint)?;
        let proofs = proofs.iter().map(|p| p.as_ref().deref().clone()).collect();

        Ok(Self {
            inner: MintProofsSdk { mint, proofs },
        })
    }

    pub fn url(&self) -> String {
        self.inner.mint.to_string()
    }

    pub fn proofs(&self) -> Vec<Arc<Proof>> {
        self.inner
            .proofs
            .clone()
            .into_iter()
            .map(|p| Arc::new(p.into()))
            .collect()
    }
}

impl From<&MintProofs> for MintProofsSdk {
    fn from(mint_proofs: &MintProofs) -> MintProofsSdk {
        mint_proofs.inner.clone()
    }
}

impl From<MintProofsSdk> for MintProofs {
    fn from(inner: MintProofsSdk) -> MintProofs {
        MintProofs { inner }
    }
}
