//! Run a Signatory in a embedded environment, inside a CDK instance, but this wrapper makes sure to
//! run the Signatory in another thread, isolated form the main CDK, communicating through messages
use std::future::Future;
use std::sync::Arc;

use cdk_common::work_pool::Admission;
use cdk_common::{BlindSignature, BlindedMessage, Error, Proof};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

use crate::signatory::{
    ReconstructDleqArguments, RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets,
};

type DynSignatory = Arc<dyn Signatory + Send + Sync>;

enum Request {
    BlindSign {
        blinded_messages: Vec<BlindedMessage>,
        admission: Admission,
        response: oneshot::Sender<Result<Vec<BlindSignature>, Error>>,
    },
    VerifyProof((Vec<Proof>, oneshot::Sender<Result<(), Error>>)),
    Keysets(oneshot::Sender<Result<SignatoryKeysets, Error>>),
    SubscribeKeysets(oneshot::Sender<Result<watch::Receiver<SignatoryKeysets>, Error>>),
    RotateKeyset(
        (
            RotateKeyArguments,
            oneshot::Sender<Result<SignatoryKeySet, Error>>,
        ),
    ),
    ReconstructDleq(
        (
            Vec<ReconstructDleqArguments>,
            oneshot::Sender<Result<Vec<BlindSignature>, Error>>,
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

    /// Serve requests in arrival order.
    ///
    /// Signing, verification and DLEQ reconstruction run in their own tasks, so
    /// a costly batch does not hold up the requests queued behind it; the signatory bounds how many
    /// of them run at once, and refuses the bounded ones it cannot take soon.
    ///
    /// The runner owns those tasks: when [`Service`] is dropped and aborts the
    /// runner, every task still waiting on the signatory is aborted with it.
    /// A job already running on the blocking pool cannot be stopped; it runs to
    /// completion, its result is discarded, and its work-pool slot is released
    /// when it ends.
    #[tracing::instrument(skip_all)]
    async fn runner(mut receiver: mpsc::Receiver<Request>, handler: DynSignatory) {
        let mut jobs = JoinSet::new();
        loop {
            let request = tokio::select! {
                Some(finished) = jobs.join_next(), if !jobs.is_empty() => {
                    if let Err(err) = finished {
                        tracing::error!("Signatory job failed: {}", err);
                    }
                    continue;
                }
                request = receiver.recv() => request,
            };
            let request = match request {
                Some(request) => request,
                None => break,
            };

            match request {
                Request::BlindSign {
                    blinded_messages,
                    admission,
                    response,
                } => {
                    let handler = handler.clone();
                    jobs.spawn(async move {
                        Self::respond(response, handler.blind_sign(blinded_messages, admission))
                            .await;
                    });
                }
                Request::VerifyProof((proofs, response)) => {
                    let handler = handler.clone();
                    jobs.spawn(async move {
                        Self::respond(response, handler.verify_proofs(proofs)).await;
                    });
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
                Request::ReconstructDleq((args, response)) => {
                    let handler = handler.clone();
                    jobs.spawn(async move {
                        Self::respond(response, handler.reconstruct_dleq(args)).await;
                    });
                }
            }
        }
    }

    async fn respond<T>(mut response: oneshot::Sender<T>, work: impl Future<Output = T>) {
        tokio::select! {
            // Skip admission if the caller has already gone away.
            biased;
            _ = response.closed() => {}
            output = work => {
                let _ = response.send(output);
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
        admission: Admission,
    ) -> Result<Vec<BlindSignature>, Error> {
        let (tx, rx) = oneshot::channel();
        self.pipeline
            .send(Request::BlindSign {
                blinded_messages,
                admission,
                response: tx,
            })
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

    #[tracing::instrument(skip_all)]
    async fn reconstruct_dleq(
        &self,
        args: Vec<ReconstructDleqArguments>,
    ) -> Result<Vec<BlindSignature>, Error> {
        let (tx, rx) = oneshot::channel();

        self.pipeline
            .send(Request::ReconstructDleq((args, tx)))
            .await
            .map_err(|e| Error::SendError(e.to_string()))?;

        rx.await.map_err(|e| Error::RecvError(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use std::time::Duration;

    use cdk_common::secret::Secret;
    use cdk_common::{Amount, Id, SecretKey};
    use tokio::sync::Notify;

    use super::*;

    /// Verification stalls until released; signing stalls forever once a
    /// `signing` sender is installed; every other call answers at once.
    struct StalledVerifier {
        started: Notify,
        release: Notify,
        signing: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    }

    impl StalledVerifier {
        fn new() -> Self {
            Self {
                started: Notify::new(),
                release: Notify::new(),
                signing: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl Signatory for StalledVerifier {
        fn name(&self) -> String {
            "Stalled".to_owned()
        }

        async fn blind_sign(
            &self,
            _: Vec<BlindedMessage>,
            _: Admission,
        ) -> Result<Vec<BlindSignature>, Error> {
            let stall = self.signing.lock().expect("signing lock").take();
            match stall {
                Some(held) => {
                    // Held until this future is dropped.
                    let _held = held;
                    self.started.notify_one();
                    std::future::pending().await
                }
                None => Ok(Vec::new()),
            }
        }

        async fn verify_proofs(&self, _: Vec<Proof>) -> Result<(), Error> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(())
        }

        async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
            Err(Error::Internal)
        }

        async fn subscribe_keysets(&self) -> Result<watch::Receiver<SignatoryKeysets>, Error> {
            Err(Error::Internal)
        }

        async fn rotate_keyset(&self, _: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
            Err(Error::Internal)
        }

        async fn reconstruct_dleq(
            &self,
            _: Vec<ReconstructDleqArguments>,
        ) -> Result<Vec<BlindSignature>, Error> {
            Err(Error::Internal)
        }
    }

    #[tokio::test]
    async fn verification_does_not_block_the_queue() {
        let handler = Arc::new(StalledVerifier::new());
        let service = Arc::new(Service::new(handler.clone()));

        let keyset = Id::from_str("009a1f293253e41e").unwrap();
        let proof = Proof::new(
            Amount::from(1),
            keyset,
            Secret::generate(),
            SecretKey::generate().public_key(),
        );
        let verifying = tokio::spawn({
            let service = service.clone();
            async move { service.verify_proofs(vec![proof]).await }
        });
        handler.started.notified().await;

        // A request queued behind the stalled verification is still served.
        tokio::time::timeout(
            Duration::from_secs(5),
            service.blind_sign(Vec::new(), Admission::Unbounded),
        )
        .await
        .expect("blind signing waited on a verification")
        .unwrap();
        assert!(!verifying.is_finished());

        handler.release.notify_one();
        verifying.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn dropping_the_caller_cancels_pending_signing() {
        let handler = Arc::new(StalledVerifier::new());
        let (held, released) = oneshot::channel();
        *handler.signing.lock().unwrap() = Some(held);
        let service = Service::new(handler.clone());

        let signing = service.blind_sign(Vec::new(), Admission::Unbounded);
        tokio::select! {
            result = signing => panic!("stalled signing finished: {result:?}"),
            _ = handler.started.notified() => {}
        }

        tokio::time::timeout(Duration::from_secs(5), released)
            .await
            .expect("signing job outlived its caller")
            .expect_err("the stalled job never completes");
    }

    #[tokio::test]
    async fn dropping_the_service_aborts_pending_signing() {
        let handler = Arc::new(StalledVerifier::new());
        let (held, released) = oneshot::channel();
        *handler.signing.lock().unwrap() = Some(held);
        let service = Service::new(handler.clone());
        let (response, _receiver) = oneshot::channel();
        service
            .pipeline
            .send(Request::BlindSign {
                blinded_messages: Vec::new(),
                admission: Admission::Unbounded,
                response,
            })
            .await
            .expect("enqueue signing");
        handler.started.notified().await;
        drop(service);

        // The job's future, and the sender it holds, are dropped with the runner.
        tokio::time::timeout(Duration::from_secs(5), released)
            .await
            .expect("signing job outlived the service")
            .expect_err("the stalled job never completes");
    }
}
