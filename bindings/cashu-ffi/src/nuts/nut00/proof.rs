use std::ops::Deref;
use std::sync::Arc;

use cashu::nuts::nut00::Proof as ProofSdk;

use crate::types::Secret;
use crate::{Amount, Id, PublicKey};

pub struct Proof {
    inner: ProofSdk,
}

impl Deref for Proof {
    type Target = ProofSdk;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Proof {
    pub fn new(
        amount: Arc<Amount>,
        secret: Arc<Secret>,
        c: Arc<PublicKey>,
        keyset_id: Arc<Id>,
    ) -> Self {
        Self {
            inner: ProofSdk {
                amount: *amount.as_ref().deref(),
                secret: secret.as_ref().deref().clone(),
                c: c.as_ref().deref().clone(),
                keyset_id: *keyset_id.as_ref().deref(),
            },
        }
    }

    pub fn amount(&self) -> Arc<Amount> {
        Arc::new(self.inner.amount.into())
    }

    pub fn secret(&self) -> Arc<Secret> {
        Arc::new(self.inner.secret.clone().into())
    }

    pub fn c(&self) -> Arc<PublicKey> {
        Arc::new(self.inner.c.clone().into())
    }

    pub fn keyset_id(&self) -> Arc<Id> {
        Arc::new(self.keyset_id.into())
    }
}

impl From<&Proof> for ProofSdk {
    fn from(proof: &Proof) -> ProofSdk {
        ProofSdk {
            amount: *proof.amount().as_ref().deref(),
            secret: proof.secret().as_ref().deref().clone(),
            c: proof.c().deref().into(),
            keyset_id: proof.keyset_id,
        }
    }
}

impl From<ProofSdk> for Proof {
    fn from(inner: ProofSdk) -> Proof {
        Proof { inner }
    }
}

pub mod mint {
    use std::ops::Deref;
    use std::sync::Arc;

    use cashu::nuts::nut00::mint::Proof as ProofSdk;

    use crate::types::Secret;
    use crate::{Amount, Id, PublicKey};

    pub struct Proof {
        inner: ProofSdk,
    }

    impl Deref for Proof {
        type Target = ProofSdk;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl Proof {
        pub fn new(
            amount: Option<Arc<Amount>>,
            secret: Arc<Secret>,
            c: Option<Arc<PublicKey>>,
            keyset_id: Option<Arc<Id>>,
        ) -> Self {
            Self {
                inner: ProofSdk {
                    amount: amount.map(|a| *a.as_ref().deref()),
                    secret: secret.as_ref().deref().clone(),
                    c: c.map(|c| c.as_ref().into()),
                    keyset_id: keyset_id.map(|id| *id.as_ref().deref()),
                },
            }
        }

        pub fn amount(&self) -> Option<Arc<Amount>> {
            self.inner.amount.map(|a| Arc::new(a.into()))
        }

        pub fn secret(&self) -> Arc<Secret> {
            Arc::new(self.inner.secret.clone().into())
        }

        pub fn c(&self) -> Option<Arc<PublicKey>> {
            self.inner.c.clone().map(|c| Arc::new(c.into()))
        }

        pub fn keyset_id(&self) -> Option<Arc<Id>> {
            self.inner.keyset_id.map(|id| Arc::new(id.into()))
        }
    }

    impl From<ProofSdk> for Proof {
        fn from(proof: ProofSdk) -> Proof {
            Proof { inner: proof }
        }
    }

    impl From<&Proof> for ProofSdk {
        fn from(proof: &Proof) -> ProofSdk {
            proof.inner.clone()
        }
    }
}
