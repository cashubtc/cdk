use std::{ops::Deref, sync::Arc};

use cashu::nuts::nut09::{MintInfo as MintInfoSdk, MintVersion as MintVersionSdk};

use crate::PublicKey;

pub struct MintVersion {
    inner: MintVersionSdk,
}

impl MintVersion {
    pub fn new(name: String, version: String) -> Self {
        Self {
            inner: MintVersionSdk { name, version },
        }
    }

    pub fn name(&self) -> String {
        self.inner.name.clone()
    }

    pub fn version(&self) -> String {
        self.inner.version.clone()
    }
}

impl From<&MintVersion> for MintVersionSdk {
    fn from(mint_version: &MintVersion) -> MintVersionSdk {
        mint_version.inner.clone()
    }
}

impl From<MintVersionSdk> for MintVersion {
    fn from(inner: MintVersionSdk) -> MintVersion {
        MintVersion { inner }
    }
}

impl Deref for MintVersion {
    type Target = MintVersionSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub struct MintInfo {
    inner: MintInfoSdk,
}

impl MintInfo {
    pub fn new(
        name: Option<String>,
        pubkey: Option<Arc<PublicKey>>,
        version: Option<Arc<MintVersion>>,
        description: Option<String>,
        description_long: Option<String>,
        contact: Vec<Vec<String>>,
        nuts: Vec<String>,
        motd: Option<String>,
    ) -> Self {
        let pubkey = pubkey.map(|p| p.as_ref().deref().clone());

        Self {
            inner: MintInfoSdk {
                name,
                pubkey,
                version: version.map(|v| v.deref().into()),
                description,
                description_long,
                contact,
                nuts,
                motd,
            },
        }
    }
}
