//! Run a Signatory in a embedded environment, inside a CDK instance, but this wrapper makes sure to
//! run the Signatory in another thread, isolated form the main CDK, communicating through messages
use std::sync::Arc;

use cdk_common::{BlindSignature, BlindedMessage, Error, Proof};
use tokio::sync::{mpsc, oneshot};
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
    Keysets(oneshot::Sender<Result<SignatoryKeysets, Error>>),
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
                    if let Err(err) = response.send(output) {
                        tracing::error!("Error sending response: {:?}", err);
                    }
                }
                Request::VerifyProof((proof, response)) => {
                    let output = handler.verify_proofs(proof).await;
                    if let Err(err) = response.send(output) {
                        tracing::error!("Error sending response: {:?}", err);
                    }
                }
                Request::Keysets(response) => {
                    let output = handler.keysets().await;
                    if let Err(err) = response.send(output) {
                        tracing::error!("Error sending response: {:?}", err);
                    }
                }
                Request::RotateKeyset((args, response)) => {
                    let output = handler.rotate_keyset(args).await;
                    if let Err(err) = response.send(output) {
                        tracing::error!("Error sending response: {:?}", err);
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
    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::Keysets(tx))
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
