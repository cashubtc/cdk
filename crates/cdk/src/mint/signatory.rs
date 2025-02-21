//! Signatory manager for handling signatory requests.
use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::DerivationPath;
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::signatory::{KeysetIdentifier, Signatory};
use cdk_common::{
    BlindSignature, BlindedMessage, CurrencyUnit, Id, KeySet, KeysResponse, KeysetResponse, Proof,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

macro_rules! signatory_manager {
    (
        $(
            $variant:ident($($input:ty),*) -> $output:ty,
        )* $(,)?
    ) => {
        paste::paste! {
        #[allow(unused_parens)]
        enum Request {
            $(
                /// Asynchronous method to handle the `[<$variant:camel>]` request.
                [<$variant:camel>]((($($input),*), oneshot::Sender<Result<$output, Error>>)),
            )*
        }

        /// Manager for handling signatory requests.
        pub struct SignatoryManager {
            inner: Arc<dyn Signatory + Send + Sync + 'static>,
            pipeline: mpsc::Sender<Request>,
            runner: JoinHandle<()>,
        }

        impl ::std::ops::Deref for SignatoryManager {
            type Target = Arc<dyn Signatory + Send + Sync + 'static>;

            fn deref(&self) -> &Self::Target {
                return &self.inner;
            }
        }

        #[allow(unused_parens)]
        impl SignatoryManager {
            /// Creates a new SignatoryManager with the given signatory.
            ///
            /// # Arguments
            /// * `signatory` - An `Arc` of a signatory object implementing the required trait.
            pub fn new(signatory: Arc<dyn Signatory + Send + Sync + 'static>) -> Self {
                let (sender, receiver) = mpsc::channel(10_000);
                let signatory_for_inner = signatory.clone();
                let runner = tokio::spawn(async move {
                    let mut receiver = receiver;
                    loop {
                        let request = if let Some(request) = receiver.recv().await {
                            request
                        } else {
                            continue;
                        };
                        let signatory = signatory.clone();
                        tokio::spawn(async move {
                            match request {
                                $(
                                    Request::[<$variant:camel>]((( $([<$input:snake>]),* ), response)) => {
                                        let output = signatory.[<$variant:lower>]($([<$input:snake>]),*).await;
                                        if let Err(err) = response.send(output) {
                                            tracing::error!("Error sending response: {:?}", err);
                                        }
                                    }
                                )*
                            }
                        });
                    }
                });

                Self {
                    pipeline: sender,
                    inner: signatory_for_inner,
                    runner,
                }
            }

            $(
                /// Asynchronous method to handle the `$variant` request.
                ///
                /// # Arguments
                /// * $($input: $input),* - The inputs required for the `$variant` request.
                ///
                /// # Returns
                /// * `Result<$output, Error>` - The result of processing the request.
                pub async fn [<$variant:lower>](&self, $([<$input:snake>]: $input),*) -> Result<$output, Error> {
                    let (sender, receiver) = oneshot::channel();

                    self.pipeline
                        .try_send(Request::[<$variant:camel>]((($([<$input:snake>]),*), sender)))
                        .map_err(|e| Error::SendError(e.to_string()))?;

                    receiver
                        .await
                        .map_err(|e| Error::RecvError(e.to_string()))?
                }
            )*
        }

        impl Drop for SignatoryManager {
            fn drop(&mut self) {
                self.runner.abort();
            }
        }

        impl<T: Signatory + Send + Sync + 'static> From<T> for SignatoryManager {
            fn from(signatory: T) -> Self {
                Self::new(Arc::new(signatory))
            }
        }

        }
    };
}

type Map = HashMap<CurrencyUnit, DerivationPath>;

signatory_manager! {
    blind_sign(BlindedMessage) -> BlindSignature,
    verify_proof(Proof) -> (),
    keyset(Id) -> Option<KeySet>,
    keysets() -> KeysetResponse,
    keyset_pubkeys(Id) -> KeysResponse,
    pubkeys() -> KeysResponse,
    rotate_keyset(CurrencyUnit, u32, u8, u64, Map) -> MintKeySetInfo,
    get_keyset_info(KeysetIdentifier) -> MintKeySetInfo,
}
