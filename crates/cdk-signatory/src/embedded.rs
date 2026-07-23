//! Run a Signatory in a embedded environment, inside a CDK instance, but this wrapper makes sure to
//! run the Signatory in another thread, isolated form the main CDK, communicating through messages
use std::sync::Arc;

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
    /// Extra signatory-side background tasks (for example keyset
    /// auto-rotation). Their lifetime is bound to this service and they are
    /// aborted when it is dropped, so they shut down with the service instead of
    /// lingering.
    background_tasks: Vec<JoinHandle<()>>,
}

impl Drop for Service {
    fn drop(&mut self) {
        if let Some(runner) = self.runner.take() {
            runner.abort();
        }
        for task in self.background_tasks.drain(..) {
            task.abort();
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
            background_tasks: Vec::new(),
        }
    }

    /// Bind a background task's lifetime to this service. The task is aborted
    /// when the service is dropped, so signatory-side work (such as keyset
    /// auto-rotation) stops with the service rather than outliving it.
    pub fn with_background_task(mut self, handle: JoinHandle<()>) -> Self {
        self.background_tasks.push(handle);
        self
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
                Request::SubscribeKeysets(response) => {
                    let output = handler.subscribe_keysets().await;
                    if response.send(output).is_err() {
                        tracing::error!("Error sending keyset subscription");
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
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::db_signatory::DbSignatory;

    #[tokio::test]
    async fn drop_aborts_background_tasks() {
        let store = Arc::new(
            cdk_sqlite::mint::memory::empty()
                .await
                .expect("in-memory db"),
        );
        let signatory = Arc::new(
            DbSignatory::new(
                store,
                b"embedded-drop-test",
                Default::default(),
                Default::default(),
            )
            .await
            .expect("DbSignatory::new"),
        );

        // A task that parks forever while holding a strong Arc we can observe
        // through a weak handle. When the task is aborted its future is dropped,
        // releasing the Arc, so `upgrade()` starts returning `None`.
        let sentinel = Arc::new(());
        let weak = Arc::downgrade(&sentinel);
        let handle = tokio::spawn(async move {
            let _held = sentinel;
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        });

        let service = Service::new(signatory).with_background_task(handle);
        assert!(
            weak.upgrade().is_some(),
            "the task holds the sentinel while running"
        );

        drop(service);

        // Give the runtime a chance to process the abort.
        for _ in 0..50 {
            if weak.upgrade().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            weak.upgrade().is_none(),
            "dropping the service must abort its background tasks"
        );
    }
}
