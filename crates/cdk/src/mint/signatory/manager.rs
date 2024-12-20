use std::collections::HashMap;
use std::sync::Arc;

use bitcoin::bip32::DerivationPath;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::{
    BlindSignature, BlindedMessage, CurrencyUnit, Error, Id, KeySet, KeysResponse, KeysetResponse,
    Proof, Signatory,
};

macro_rules! signatory_manager {
    (
        $(
            $variant:ident($($input:ty),*) -> $output:ty,
        )* $(,)?
    ) => {
        paste::paste! {
        #[allow(non_camel_case_types, unused_parens)]
        enum Request {
            $(
                $variant((($($input),*), oneshot::Sender<Result<$output, Error>>)),
            )*
        }

        /// Manager for handling signatory requests.
        pub struct SignatoryManager {
            pipeline: mpsc::Sender<Request>,
            runner: JoinHandle<()>,
        }

        #[allow(non_camel_case_types, unused_parens, non_snake_case)]
        impl SignatoryManager {
            /// Creates a new SignatoryManager with the given signatory.
            ///
            /// # Arguments
            /// * `signatory` - An `Arc` of a signatory object implementing the required trait.
            pub fn new(signatory: Arc<dyn Signatory + Send + Sync + 'static>) -> Self {
                let (sender, receiver) = mpsc::channel(10_000);
                let runner = tokio::spawn(async move {
                    let mut receiver = receiver;
                    loop {
                        let request = if let Some(request) = receiver.recv().await {
                            request
                        } else {
                            continue;
                        };
                        let signatory = signatory.clone();
                        match request {
                            $(
                                Request::$variant((( $($input),* ), response)) => {
                                    tokio::spawn(async move {
                                        let output = signatory.[<$variant:lower>]($($input),*).await;
                                        if let Err(err) = response.send(output) {
                                            tracing::error!("Error sending response: {:?}", err);
                                        }
                                    });
                                }
                            )*
                        }
                    }
                });

                Self {
                    pipeline: sender,
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
                pub async fn [<$variant:lower>](&self, $($input: $input),*) -> Result<$output, Error> {
                    let (sender, receiver) = oneshot::channel();

                    self.pipeline
                        .try_send(Request::$variant((($($input),*), sender)))
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
    rotate_keyset(CurrencyUnit, u32, u8, u64, Map) -> (),
}
