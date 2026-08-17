//! Run a Signatory in a embedded environment, inside a CDK instance, but this wrapper makes sure to
//! run the Signatory in another thread, isolated form the main CDK, communicating through messages
use std::sync::Arc;

use bitcoin::secp256k1::schnorr::Signature;
use cdk_common::{BlindSignature, BlindedMessage, Error, Proof};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

enum Request {
    BlindSign(
        (
            Vec<BlindedMessage>,
            oneshot::Sender<Result<Vec<BlindSignature>, Error>>,
        ),
    ),
    VerifyProof((Vec<Proof>, oneshot::Sender<Result<(), Error>>)),
    Sign((Vec<u8>, oneshot::Sender<Result<Signature, Error>>)),
    Keysets(oneshot::Sender<Result<SignatoryKeysets, Error>>),
    SubscribeKeysets(oneshot::Sender<Result<watch::Receiver<SignatoryKeysets>, Error>>),
    RotateKeyset(
        (
            RotateKeyArguments,
            oneshot::Sender<Result<SignatoryKeySet, Error>>,
        ),
    ),
}

/// Creates a service-like to wrap an implementation of the Signatory
///
/// This implements the actor model, ensuring the Signatory and their private key is moved from the
/// main thread to their own tokio task, and communicates with the main program by passing messages,
/// an extra layer of security to move the keys to another layer.
#[allow(missing_debug_implementations)]
pub struct Service {
    pipeline: mpsc::Sender<Request>,
    runner: Option<JoinHandle<()>>,
}

impl Drop for Service {
    fn drop(&mut self) {
        if let Some(runner) = self.runner.take() {
            runner.abort();
        }
    }
}

impl Service {
    /// Takes a signatory and spawns it into a Tokio task, isolating its implementation with the
    /// main thread, communicating with it through messages
    pub fn new(handler: Arc<dyn Signatory + Send + Sync>) -> Self {
        let (tx, rx) = mpsc::channel(10_000);
        let runner = Some(tokio::spawn(Self::runner(rx, handler)));

        Self {
            pipeline: tx,
            runner,
        }
    }

    #[tracing::instrument(skip_all)]
    async fn runner(
        mut receiver: mpsc::Receiver<Request>,
        handler: Arc<dyn Signatory + Send + Sync>,
    ) {
        while let Some(request) = receiver.recv().await {
            match request {
                Request::BlindSign((blinded_message, response)) => {
                    let output = handler.blind_sign(blinded_message).await;
                    if response.send(output).is_err() {
                        tracing::error!("Error sending blind-sign response: receiver dropped");
                    }
                }
                Request::VerifyProof((proof, response)) => {
                    let output = handler.verify_proofs(proof).await;
                    if response.send(output).is_err() {
                        tracing::error!(
                            "Error sending proof-verification response: receiver dropped"
                        );
                    }
                }
                Request::Sign((payload, response)) => {
                    let output = handler.sign(payload).await;
                    if response.send(output).is_err() {
                        tracing::error!("Error sending sign response: receiver dropped");
                    }
                }
                Request::Keysets(response) => {
                    let output = handler.keysets().await;
                    if response.send(output).is_err() {
                        tracing::error!("Error sending keysets response: receiver dropped");
                    }
                }
                Request::SubscribeKeysets(response) => {
                    let output = handler.subscribe_keysets().await;
                    if response.send(output).is_err() {
                        tracing::error!("Error sending keyset subscription");
                    }
                }
                Request::RotateKeyset((args, response)) => {
                    let output = handler.rotate_keyset(args).await;
                    if response.send(output).is_err() {
                        tracing::error!("Error sending keyset-rotation response: receiver dropped");
                    }
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl Signatory for Service {
    fn name(&self) -> String {
        "Embedded".to_owned()
    }

    #[tracing::instrument(skip_all)]
    async fn blind_sign(
        &self,
        blinded_messages: Vec<BlindedMessage>,
    ) -> Result<Vec<BlindSignature>, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::BlindSign((blinded_messages, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    #[tracing::instrument(skip_all)]
    async fn verify_proofs(&self, proofs: Vec<Proof>) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::VerifyProof((proofs, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    #[tracing::instrument(skip_all)]
    async fn sign(&self, payload: Vec<u8>) -> Result<Signature, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::Sign((payload, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    #[tracing::instrument(skip_all)]
    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::Keysets(tx))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    #[tracing::instrument(skip_all)]
    async fn subscribe_keysets(&self) -> Result<watch::Receiver<SignatoryKeysets>, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::SubscribeKeysets(tx))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    #[tracing::instrument(skip(self))]
    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::RotateKeyset((args, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::db_signatory::DbSignatory;
    use crate::identity;

    #[tokio::test]
    async fn sign_round_trips_through_the_actor() {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let inner = DbSignatory::new(
            store,
            b"test-seed-for-embedded-signing",
            Default::default(),
            Default::default(),
        )
        .await
        .expect("DbSignatory::new");

        let service = Service::new(Arc::new(inner));

        let payload = b"an arbitrary stream of bytes".to_vec();
        let signature = service.sign(payload.clone()).await.expect("sign");

        let pubkey = service.keysets().await.expect("keysets").pubkey;
        identity::verify(&pubkey, &payload, &signature)
            .expect("signature must verify against the published pubkey");
    }
}
