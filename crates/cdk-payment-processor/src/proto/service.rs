use cdk_common::payment::DynMintPayment;

#[derive(Clone)]
pub(crate) struct PaymentProcessorService {
    pub(super) inner: DynMintPayment,
}

impl std::fmt::Debug for PaymentProcessorService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaymentProcessorService")
            .finish_non_exhaustive()
    }
}

impl PaymentProcessorService {
    pub(crate) fn new(inner: DynMintPayment) -> Self {
        Self { inner }
    }

    pub(crate) async fn start(&self) -> Result<(), cdk_common::payment::Error> {
        self.inner.start().await
    }

    pub(crate) async fn stop(&self) -> Result<(), cdk_common::payment::Error> {
        self.inner.stop().await
    }

    pub(crate) fn cancel_payment_event_stream(&self) {
        self.inner.cancel_payment_event_stream();
    }
}
