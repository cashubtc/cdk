use std::ops::Deref;
use std::sync::Arc;

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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: Option<String>,
        pubkey: Option<Arc<PublicKey>>,
        version: Option<Arc<MintVersion>>,
        description: Option<String>,
        description_long: Option<String>,
        contact: Option<Vec<Vec<String>>>,
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

    pub fn name(&self) -> Option<String> {
        self.inner.name.clone()
    }

    pub fn pubkey(&self) -> Option<Arc<PublicKey>> {
        self.inner.pubkey.clone().map(|p| Arc::new(p.into()))
    }

    pub fn version(&self) -> Option<Arc<MintVersion>> {
        self.inner.version.clone().map(|v| Arc::new(v.into()))
    }

    pub fn description(&self) -> Option<String> {
        self.inner.description.clone()
    }

    pub fn description_long(&self) -> Option<String> {
        self.inner.description_long.clone()
    }

    pub fn contact(&self) -> Option<Vec<Vec<String>>> {
        self.inner.contact.clone()
    }

    pub fn nuts(&self) -> Vec<String> {
        self.inner.nuts.clone()
    }

    pub fn motd(&self) -> Option<String> {
        self.inner.motd.clone()
    }
}

impl From<cashu::nuts::nut09::MintInfo> for MintInfo {
    fn from(inner: cashu::nuts::nut09::MintInfo) -> MintInfo {
        MintInfo { inner }
    }
}
