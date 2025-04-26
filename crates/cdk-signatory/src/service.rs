use std::sync::Arc;

use cashu::{BlindSignature, BlindedMessage, Proof};
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet};

enum Request {
    BlindSign(
        (
            BlindedMessage,
            oneshot::Sender<Result<BlindSignature, Error>>,
        ),
    ),
    VerifyProof((Proof, oneshot::Sender<Result<(), Error>>)),
    Keysets(oneshot::Sender<Result<Vec<SignatoryKeySet>, Error>>),
    RotateKeyset(
        (
            RotateKeyArguments,
            oneshot::Sender<Result<MintKeySetInfo, Error>>,
        ),
    ),
}

/// Creates a service-like to wrap an implementation of the Signatory
///
/// This implements the actor model, ensuring the Signatory and their private key is moved from the
/// main thread to their own tokio task, and communicates with the main program by passing messages,
/// an extra layer of security to move the keys to another layer.
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
    pub fn new(handler: Arc<dyn Signatory + Send + Sync>) -> Self {
        let (tx, rx) = mpsc::channel(10_000);
        let runner = Some(tokio::spawn(Self::runner(rx, handler)));

        Self {
            pipeline: tx,
            runner,
        }
    }

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
                    let output = handler.verify_proof(proof).await;
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
    async fn blind_sign(&self, blinded_message: BlindedMessage) -> Result<BlindSignature, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::BlindSign((blinded_message, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    async fn verify_proof(&self, proof: Proof) -> Result<(), Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::VerifyProof((proof, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    async fn keysets(&self) -> Result<Vec<SignatoryKeySet>, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::Keysets(tx))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }

    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<MintKeySetInfo, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::RotateKeyset((args, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }
}
