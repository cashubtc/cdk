use core::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::{Stream, StreamExt};
use nostr::prelude::{Keys, PublicKey};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    payment_request_method_fee_applies, payment_request_mint_policy_accepts_mint,
    wallet_payment_request_method,
};
use crate::mint_url::MintUrl;
use crate::nuts::nut00::ProofsMethods;
use crate::nuts::nut10::{Conditions, Kind, SpendingConditions};
use crate::nuts::{Nut10Secret, PaymentRequest, PaymentRequestPayload, Token};
use crate::wallet::receive::saga::ReceiveSaga;
use crate::wallet::{ReceiveOptions, WalletRepository};
use crate::{Amount, Error, Wallet};

/// A managed, memory-only Nostr payment request receiving identity.
///
/// Created by `WalletRepository::create_request`. Clones share cancellation and
/// consumption state. Keep this handle until payment completes; the ephemeral
/// identity is not persisted and cannot receive payments after an app restart.
#[derive(Debug, Clone)]
pub struct NostrWaitInfo {
    /// Ephemeral keys used to unwrap incoming gift wraps.
    pub keys: Keys,
    /// Relays hosting the request's inbox.
    pub relays: Vec<String>,
    /// Recipient public key.
    pub pubkey: PublicKey,
    /// Accepted or preferred mints.
    pub mints: Vec<MintUrl>,
    /// Whether the mint list is advisory.
    pub mint_preferred: Option<bool>,
    request: PaymentRequest,
    cancel: CancellationToken,
    state: Arc<Mutex<ReceiveState>>,
}

#[derive(Default)]
struct ReceiveState {
    consumed: AtomicBool,
    payload: Option<PaymentRequestPayload>,
}

impl fmt::Debug for ReceiveState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReceiveState")
            .field("consumed", &self.consumed)
            .field("has_payload", &self.payload.is_some())
            .finish()
    }
}

impl NostrWaitInfo {
    pub(super) fn new(keys: Keys, relays: Vec<String>, request: PaymentRequest) -> Self {
        Self {
            pubkey: keys.public_key(),
            keys,
            relays,
            mints: request.mints.clone(),
            mint_preferred: request.mint_preferred,
            request,
            cancel: CancellationToken::new(),
            state: Arc::new(Mutex::new(ReceiveState::default())),
        }
    }

    /// Permanently cancel listening on this handle and all of its clones.
    ///
    /// An already-started redemption is allowed to finish and report its result.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub(super) async fn receive(
        &self,
        repository: &WalletRepository,
        options: ReceiveOptions,
    ) -> Result<Amount, Error> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use crate::wallet::streams::nostr::NostrPaymentEventStream;

            let stream =
                NostrPaymentEventStream::new(self.keys.clone(), self.relays.clone(), self.pubkey);
            self.receive_stream(repository, stream, options).await
        }
        #[cfg(target_arch = "wasm32")]
        {
            use nostr::prelude::{Filter, Kind, UnwrappedGift};
            use nostr_sdk::prelude::{
                Client, ClientNotification, RelayCapabilities, SignerAuthenticator,
            };

            let client = Client::builder()
                .authenticator(SignerAuthenticator::new(self.keys.clone()))
                .build();
            let notifications = client.notifications();
            let stream = futures::stream::once(async {
                for relay in &self.relays {
                    client
                        .add_relay(relay)
                        .capabilities(RelayCapabilities::READ)
                        .await
                        .map_err(|e| Error::Custom(format!("Add relay: {e}")))?;
                }
                client.connect().await;
                client
                    .subscribe(Filter::new().pubkey(self.pubkey).kind(Kind::GiftWrap))
                    .await
                    .map_err(|e| Error::Custom(format!("Subscribe: {e}")))?;
                Ok(())
            })
            .filter_map(|result| async { result.err().map(Err) })
            .chain(notifications.filter_map(|notification| async move {
                match notification {
                    ClientNotification::Event { event, .. } => {
                        let gift = UnwrappedGift::from_gift_wrap(&self.keys, &event).ok()?;
                        serde_json::from_str(&gift.rumor.content).ok().map(Ok)
                    }
                    _ => None,
                }
            }));
            let result = self.receive_stream(repository, stream, options).await;
            client.disconnect().await;
            result
        }
    }

    async fn receive_stream<S>(
        &self,
        repository: &WalletRepository,
        stream: S,
        options: ReceiveOptions,
    ) -> Result<Amount, Error>
    where
        S: Stream<Item = Result<PaymentRequestPayload, Error>>,
    {
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| Error::PaymentRequestReceiveInProgress)?;
        if state.consumed.load(Ordering::Relaxed) {
            return Err(Error::PaymentRequestReceiveConsumed);
        }
        let mut stream = Box::pin(stream);
        loop {
            let item = match &state.payload {
                Some(payload) => Ok(payload.clone()),
                None => tokio::select! {
                    biased;
                    _ = self.cancel.cancelled() => return Err(Error::PaymentRequestReceiveCancelled),
                    item = stream.next() => item.ok_or(Error::PaymentRequestReceiveClosed)?,
                },
            };
            // Malformed or unrelated relay messages must not end a pending request.
            let payload = match item {
                Ok(payload) if self.matches_payload(&payload) => payload,
                Ok(_) | Err(Error::InvalidPaymentRequest) => continue,
                Err(error) => return Err(error),
            };
            let wallet = tokio::select! {
                biased;
                _ = self.cancel.cancelled() => return Err(Error::PaymentRequestReceiveCancelled),
                wallet = self.accept_payment(repository, &payload) => wallet?,
            };
            let Some(wallet) = wallet else {
                state.payload = None;
                continue;
            };
            state.payload = Some(payload.clone());
            let token = Token::new(payload.mint, payload.proofs, payload.memo, payload.unit);
            let proofs = wallet.token_proofs(&token).await?;
            let result = wallet
                .retry_on_inactive_keyset(|| async {
                    let saga = ReceiveSaga::new(&wallet)
                        .prepare(
                            proofs.clone(),
                            options.clone(),
                            token.memo().clone(),
                            Some(token.to_string()),
                        )
                        .await?;
                    let saga = saga
                        .execute_with_recovery_callback(|| {
                            // Recovery owns the payment from this point, including
                            // if the caller drops the receive future.
                            state.consumed.store(true, Ordering::Relaxed);
                        })
                        .await?;
                    Ok(saga.into_amount())
                })
                .await;
            match result {
                // These proof validation failures cannot be repaired by receive
                // options. Keep listening for a replacement payment, but never
                // discard a payment once recovery owns it.
                Err(Error::NUT12(_) | Error::AmountKey)
                    if !state.consumed.load(Ordering::Relaxed) =>
                {
                    state.payload = None;
                }
                result => return result,
            }
        }
    }

    fn matches_payload(&self, payload: &PaymentRequestPayload) -> bool {
        if self.request.payment_id.is_some() && self.request.payment_id != payload.id {
            return false;
        }
        if self
            .request
            .unit
            .as_ref()
            .is_some_and(|unit| *unit != payload.unit)
            || payload.proofs.is_empty()
            || !payment_request_mint_policy_accepts_mint(
                &self.request.mints,
                self.request.mint_preferred,
                &payload.mint,
            )
        {
            return false;
        }
        if let Some(required) = &self.request.nut10 {
            let required_conditions = normalize_conditions(required.tags.clone());
            if required_conditions.is_none() {
                return false;
            }
            for proof in &payload.proofs {
                let Ok(secret) = Nut10Secret::try_from(&proof.secret) else {
                    return false;
                };
                if SpendingConditions::try_from(secret.clone())
                    .and_then(|conditions| conditions.validate())
                    .is_err()
                {
                    return false;
                }
                let data_matches = match required.kind {
                    Kind::P2PK => {
                        use crate::nuts::nut01::PublicKey;

                        match (
                            PublicKey::from_hex(secret.secret_data().data()),
                            PublicKey::from_hex(&required.data),
                        ) {
                            (Ok(actual), Ok(required)) => {
                                actual.x_only_public_key() == required.x_only_public_key()
                            }
                            _ => false,
                        }
                    }
                    Kind::HTLC => secret
                        .secret_data()
                        .data()
                        .eq_ignore_ascii_case(&required.data),
                };
                if secret.kind() != required.kind
                    || !data_matches
                    || normalize_conditions(secret.secret_data().tags().cloned())
                        != required_conditions
                {
                    return false;
                }
            }
        }
        true
    }

    async fn accept_payment(
        &self,
        repository: &WalletRepository,
        payload: &PaymentRequestPayload,
    ) -> Result<Option<Wallet>, Error> {
        let wallet = repository
            .get_or_create_wallet(payload.mint.clone(), payload.unit.clone(), None)
            .await?;
        if let Some(required) = &self.request.nut10 {
            let nuts = wallet.load_mint_info().await?.nuts;
            let supported = match required.kind {
                Kind::P2PK => nuts.nut11.supported,
                Kind::HTLC => nuts.nut14.supported,
            };
            if !nuts.nut10.supported || !supported {
                return Ok(None);
            }
        }
        let Some(method) =
            wallet_payment_request_method(&wallet, &self.request, &payload.unit).await?
        else {
            return Ok(None);
        };
        let method_fee = if payment_request_method_fee_applies(&self.request, &payload.mint) {
            method.fee
        } else {
            Amount::ZERO
        };
        let required = self
            .request
            .amount
            .unwrap_or(Amount::ZERO)
            .checked_add(method_fee)
            .ok_or(Error::AmountOverflow)?;
        let input_fee = wallet.get_proofs_fee(&payload.proofs).await?.total;
        let net = payload.proofs.total_amount()?.checked_sub(input_fee);
        if !net.is_some_and(|net| net > Amount::ZERO && net >= required) {
            return Ok(None);
        }
        // Receive verifies supplied DLEQ proofs and redeems with the mint before
        // reporting success. Missing DLEQ is permitted for this online flow.
        Ok(Some(wallet))
    }
}

fn normalize_conditions(tags: Option<Vec<Vec<String>>>) -> Option<Conditions> {
    let mut conditions = Conditions::try_from(tags.unwrap_or_default()).ok()?;
    for keys in [&mut conditions.pubkeys, &mut conditions.refund_keys]
        .into_iter()
        .flatten()
    {
        for key in keys.iter_mut() {
            *key = key
                .x_only_public_key()
                .public_key(bitcoin::secp256k1::Parity::Even)
                .into();
        }
        keys.sort();
    }
    if conditions.pubkeys.as_ref().is_some_and(Vec::is_empty) {
        conditions.pubkeys = None;
    }
    conditions.num_sigs = Some(conditions.num_sigs.unwrap_or(1));
    conditions.num_sigs_refund = Some(conditions.num_sigs_refund.unwrap_or(1));
    Some(conditions)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::time::Duration;

    use futures::{poll, stream, SinkExt};
    use nostr::prelude::{Event, FinalizeEvent, PrivateDirectMessageBuilder};
    use tokio::net::TcpListener;
    use tokio::time::timeout;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;
    use crate::dhke::{hash_to_curve, sign_message};
    use crate::nuts::{CurrencyUnit, SecretKey, SupportedMethod};
    use crate::wallet::payment_request::CreateRequestParams;
    use crate::wallet::test_utils::{
        create_test_db, test_keyset_id, test_mint_url, test_proof, MockMintConnector,
    };
    use crate::wallet::WalletConfig;

    async fn repository() -> WalletRepository {
        crate::wallet::WalletRepositoryBuilder::new()
            .localstore(create_test_db().await)
            .seed([42; 64])
            .build()
            .await
            .expect("repository")
    }

    async fn handle(repository: &WalletRepository) -> NostrWaitInfo {
        repository
            .create_request(CreateRequestParams {
                payment_id: Some("request-42".to_string()),
                transport: "nostr".to_string(),
                nostr_relays: Some(vec!["wss://relay.example.com".to_string()]),
                mints: Some(vec![test_mint_url().to_string()]),
                ..Default::default()
            })
            .await
            .expect("request")
            .1
            .expect("managed identity")
    }

    fn payload() -> PaymentRequestPayload {
        PaymentRequestPayload {
            id: Some("request-42".to_string()),
            memo: None,
            mint: test_mint_url(),
            unit: CurrencyUnit::Sat,
            proofs: vec![test_proof(test_keyset_id(), 4)],
        }
    }

    #[tokio::test]
    async fn managed_receive_checks_identity_unit_and_mint_before_network_access() {
        let repository = repository().await;
        let info = handle(&repository).await;
        let valid = payload();
        assert!(info.matches_payload(&valid));
        let mut wrong_id = valid.clone();
        wrong_id.id = None;
        let mut wrong_unit = valid.clone();
        wrong_unit.unit = CurrencyUnit::Msat;
        let mut wrong_mint = valid.clone();
        wrong_mint.mint = "https://other.example.com".parse().unwrap();
        let mut empty = valid;
        empty.proofs.clear();
        let stream = stream::iter([wrong_id, wrong_unit, wrong_mint, empty].map(Ok));
        assert!(matches!(
            info.receive_stream(&repository, stream, ReceiveOptions::default())
                .await,
            Err(Error::PaymentRequestReceiveClosed)
        ));
        assert!(repository.get_wallets().await.is_empty());
    }

    #[tokio::test]
    async fn managed_receive_cancellation_and_concurrency_share_state() {
        let repository = repository().await;
        let info = handle(&repository).await;
        let cloned = info.clone();
        let wait = info.receive_stream(&repository, stream::pending(), ReceiveOptions::default());
        tokio::pin!(wait);
        assert!(poll!(&mut wait).is_pending());
        assert!(matches!(
            cloned
                .receive_stream(&repository, stream::pending(), ReceiveOptions::default())
                .await,
            Err(Error::PaymentRequestReceiveInProgress)
        ));
        cloned.cancel();
        assert!(matches!(
            wait.await,
            Err(Error::PaymentRequestReceiveCancelled)
        ));
        assert!(matches!(
            cloned
                .receive_stream(&repository, stream::pending(), ReceiveOptions::default())
                .await,
            Err(Error::PaymentRequestReceiveCancelled)
        ));
    }

    #[tokio::test]
    async fn managed_receive_cancellation_closes_stalled_relay_connection() {
        use tokio::io::AsyncReadExt;

        timeout(Duration::from_secs(5), async {
            let repository = repository().await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let (_, info) = repository
                .create_request(CreateRequestParams {
                    transport: "nostr".to_string(),
                    nostr_relays: Some(vec![format!("ws://{}", listener.local_addr().unwrap())]),
                    ..Default::default()
                })
                .await
                .unwrap();
            let info = info.unwrap();
            let wait = repository
                .wait_for_nostr_payment_with_options(info.clone(), ReceiveOptions::default());
            tokio::pin!(wait);
            // Accepting the socket establishes that the wait is actively connecting.
            let (mut socket, _) = tokio::select! {
                connection = listener.accept() => connection.unwrap(),
                result = &mut wait => panic!("Receive ended before connecting: {result:?}"),
            };
            assert!(matches!(
                repository
                    .wait_for_nostr_payment_with_options(info.clone(), ReceiveOptions::default())
                    .await,
                Err(Error::PaymentRequestReceiveInProgress)
            ));
            info.cancel();
            assert!(matches!(
                wait.await,
                Err(Error::PaymentRequestReceiveCancelled)
            ));
            let mut bytes = Vec::new();
            socket
                .read_to_end(&mut bytes)
                .await
                .expect("cancelled connection closes");
        })
        .await
        .expect("receive cancellation must close the relay connection");
    }

    #[tokio::test]
    async fn managed_receive_matches_equivalent_p2pk_conditions() {
        let repository = repository().await;
        let key = SecretKey::generate().public_key();
        let (request, info) = repository
            .create_request(CreateRequestParams {
                payment_id: Some("request-42".to_string()),
                pubkeys: Some(vec![key.to_hex()]),
                transport: "nostr".to_string(),
                nostr_relays: Some(vec!["wss://relay.example.com".to_string()]),
                ..Default::default()
            })
            .await
            .unwrap();
        let info = info.unwrap();
        let mut condition = request.nut10.unwrap();
        // NUT-11 compares x-coordinates, and omitted threshold/sigflag mean 1/SIG_INPUTS.
        condition.data = key
            .x_only_public_key()
            .public_key(bitcoin::secp256k1::Parity::Odd)
            .to_string()
            .to_uppercase();
        condition.tags = None;
        let mut payment = payload();
        let secret: Nut10Secret = condition.clone().into();
        payment.proofs[0].secret = secret.try_into().unwrap();
        assert!(info.matches_payload(&payment));

        condition.tags = Some(vec![vec!["locktime".to_string(), "1".to_string()]]);
        let secret: Nut10Secret = condition.into();
        payment.proofs[0].secret = secret.try_into().unwrap();
        assert!(!info.matches_payload(&payment));
    }

    #[tokio::test]
    async fn managed_receive_validates_net_amount_and_method_fees() {
        let repository = repository().await;
        let mock = Arc::new(MockMintConnector::new());
        let wallet = repository
            .create_wallet(
                test_mint_url(),
                CurrencyUnit::Sat,
                Some(WalletConfig::new().with_mint_connector(mock.clone())),
            )
            .await
            .unwrap();
        let mut info = handle(&repository).await;
        info.request.amount = Some(Amount::from(2));
        info.request.supported_methods = vec![SupportedMethod::with_fee("bolt11", 2)];
        let payload = payload();
        // The mock keyset charges one unit per input: 4 - 1 = 3 net.
        assert_eq!(
            wallet.get_proofs_fee(&payload.proofs).await.unwrap().total,
            Amount::from(1)
        );
        assert!(info
            .accept_payment(&repository, &payload)
            .await
            .unwrap()
            .is_some());
        // Unlisted mints must also cover the requested method fee: 2 + 2 > 3.
        info.request.mints.clear();
        assert!(info
            .accept_payment(&repository, &payload)
            .await
            .unwrap()
            .is_none());
        info.request.supported_methods = vec![SupportedMethod::with_fee("bolt11", 1)];
        assert!(info
            .accept_payment(&repository, &payload)
            .await
            .unwrap()
            .is_some());
        info.request.supported_methods = vec![SupportedMethod::with_fee("unavailable", 0)];
        assert!(info
            .accept_payment(&repository, &payload)
            .await
            .unwrap()
            .is_none());
        assert!(mock.captured_swap_requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn managed_receive_skips_invalid_dleq_and_reads_corrected_payment() {
        for same_stream in [false, true] {
            let repository = repository().await;
            let mock = Arc::new(MockMintConnector::new());
            mock.enable_mint_signing();
            let keyset_id = mock.keysets.lock().unwrap()[0].id;
            let wallet = repository
                .create_wallet(
                    test_mint_url(),
                    CurrencyUnit::Sat,
                    Some(WalletConfig::new().with_mint_connector(mock.clone())),
                )
                .await
                .unwrap();
            let info = handle(&repository).await;
            let mut payment = payload();
            payment.proofs[0] = test_proof(keyset_id, 4);
            let mint_key =
                mock.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&Amount::from(4)].clone();
            payment.proofs[0].c = sign_message(
                &mint_key,
                &hash_to_curve(payment.proofs[0].secret.as_bytes()).unwrap(),
            )
            .unwrap();
            let mut invalid = payment.clone();
            invalid.proofs[0].dleq = Some(crate::nuts::nut12::ProofDleq::new(
                SecretKey::from_slice(&[1; 32]).unwrap(),
                SecretKey::from_slice(&[2; 32]).unwrap(),
                SecretKey::from_slice(&[3; 32]).unwrap(),
            ));
            assert!(invalid.proofs[0]
                .verify_dleq(mint_key.public_key())
                .is_err());

            let payments = match same_stream {
                true => vec![Ok(invalid), Ok(payment)],
                false => {
                    assert!(matches!(
                        info.receive_stream(
                            &repository,
                            stream::iter([Ok(invalid)]),
                            ReceiveOptions::default(),
                        )
                        .await,
                        Err(Error::PaymentRequestReceiveClosed)
                    ));
                    assert!(mock.captured_swap_requests.lock().unwrap().is_empty());
                    assert!(wallet
                        .localstore
                        .get_incomplete_sagas()
                        .await
                        .unwrap()
                        .is_empty());
                    vec![Ok(payment)]
                }
            };
            let amount = info
                .clone()
                .receive_stream(
                    &repository,
                    stream::iter(payments),
                    ReceiveOptions::default(),
                )
                .await
                .unwrap();
            assert_eq!(amount, Amount::from(4));
            assert_eq!(wallet.total_balance().await.unwrap(), amount);
            assert_eq!(mock.captured_swap_requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn managed_receive_retries_retained_htlc_after_preparation_failure() {
        for fail_swap in [false, true] {
            let repository = repository().await;
            let mock = Arc::new(MockMintConnector::new());
            mock.enable_mint_signing();
            let keyset_id = mock.keysets.lock().unwrap()[0].id;
            let wallet = repository
                .create_wallet(
                    test_mint_url(),
                    CurrencyUnit::Sat,
                    Some(WalletConfig::new().with_mint_connector(mock.clone())),
                )
                .await
                .unwrap();
            let preimage = "00".repeat(32);
            let (request, info) = repository
                .create_request(CreateRequestParams {
                    payment_id: Some("request-42".to_string()),
                    preimage: Some(preimage.clone()),
                    transport: "nostr".to_string(),
                    nostr_relays: Some(vec!["wss://relay.example.com".to_string()]),
                    mints: Some(vec![test_mint_url().to_string()]),
                    ..Default::default()
                })
                .await
                .unwrap();
            let info = info.unwrap();
            let cloned = info.clone();
            let mut payment = payload();
            payment.proofs[0] = test_proof(keyset_id, 4);
            let secret: Nut10Secret = request.nut10.unwrap().into();
            payment.proofs[0].secret = secret.try_into().unwrap();
            let mint_key =
                mock.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&Amount::from(4)].clone();
            payment.proofs[0].c = sign_message(
                &mint_key,
                &hash_to_curve(payment.proofs[0].secret.as_bytes()).unwrap(),
            )
            .unwrap();

            assert!(matches!(
                info.receive_stream(
                    &repository,
                    stream::iter([Ok(payment)]),
                    ReceiveOptions::default(),
                )
                .await,
                Err(Error::PreimageNotProvided)
            ));
            assert!(mock.captured_swap_requests.lock().unwrap().is_empty());
            assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
            assert!(wallet
                .localstore
                .get_incomplete_sagas()
                .await
                .unwrap()
                .is_empty());

            if fail_swap {
                mock.set_post_swap_response(Err(Error::Custom("swap failed".to_string())));
            }
            // A clone retries without requiring the relay to deliver the payload again.
            let result = cloned
                .receive_stream(
                    &repository,
                    stream::empty(),
                    ReceiveOptions {
                        preimages: vec![preimage],
                        ..Default::default()
                    },
                )
                .await;
            match fail_swap {
                true => assert!(
                    matches!(result, Err(Error::Custom(message)) if message == "swap failed")
                ),
                false => {
                    assert_eq!(result.unwrap(), Amount::from(4));
                    assert_eq!(wallet.total_balance().await.unwrap(), Amount::from(4));
                }
            }
            assert_eq!(mock.captured_swap_requests.lock().unwrap().len(), 1);
            // Once recovery state exists, neither success nor a mint error allows replay.
            assert!(matches!(
                info.receive_stream(&repository, stream::empty(), ReceiveOptions::default())
                    .await,
                Err(Error::PaymentRequestReceiveConsumed)
            ));
        }
    }

    // A small local relay sends real NIP-17 encrypted payloads after subscription.
    async fn relay(listener: TcpListener, events: Vec<Event>) {
        let (socket, _) = listener.accept().await.expect("relay connection");
        let mut socket = accept_async(socket).await.expect("WebSocket handshake");
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Text(text) = message {
                let message: serde_json::Value =
                    serde_json::from_str(&text).expect("relay message");
                if message[0] == "REQ" {
                    for event in &events {
                        socket
                            .send(Message::Text(
                                serde_json::json!(["EVENT", message[1], event])
                                    .to_string()
                                    .into(),
                            ))
                            .await
                            .expect("deliver event");
                    }
                    socket
                        .send(Message::Text(
                            serde_json::json!(["EOSE", message[1]]).to_string().into(),
                        ))
                        .await
                        .expect("end stored events");
                }
            }
        }
    }

    #[tokio::test]
    async fn managed_receive_decrypts_validates_and_redeems_locked_payments() {
        for htlc in [false, true] {
            let repository = repository().await;
            let mock = Arc::new(MockMintConnector::new());
            mock.enable_mint_signing();
            let keyset_id = mock.keysets.lock().unwrap()[0].id;
            let wallet = repository
                .create_wallet(
                    test_mint_url(),
                    CurrencyUnit::Sat,
                    Some(WalletConfig::new().with_mint_connector(mock.clone())),
                )
                .await
                .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let signing_key = SecretKey::generate();
            let preimage = "00".repeat(32);
            let (request, info) = repository
                .create_request(CreateRequestParams {
                    amount: Some(4),
                    payment_id: Some("request-42".to_string()),
                    pubkeys: Some(vec![signing_key.public_key().to_string()]),
                    preimage: htlc.then(|| preimage.clone()),
                    transport: "nostr".to_string(),
                    nostr_relays: Some(vec![format!("ws://{}", listener.local_addr().unwrap())]),
                    mints: Some(vec![test_mint_url().to_string()]),
                    ..Default::default()
                })
                .await
                .unwrap();
            let info = info.unwrap();
            let mut payment = payload();
            payment.proofs[0] = test_proof(keyset_id, 4);
            let secret: Nut10Secret = request.nut10.unwrap().into();
            payment.proofs[0].secret = secret.try_into().unwrap();
            let mint_key =
                mock.mint_signing_keys.lock().unwrap().as_ref().unwrap()[&Amount::from(4)].clone();
            payment.proofs[0].c = sign_message(
                &mint_key,
                &hash_to_curve(payment.proofs[0].secret.as_bytes()).unwrap(),
            )
            .unwrap();
            let mut unlocked = payment.clone();
            unlocked.proofs[0].secret = test_proof(keyset_id, 4).secret;
            assert!(!info.matches_payload(&unlocked));
            let mut underpaid = payment.clone();
            underpaid.proofs[0].amount = Amount::from(2);
            let sender = Keys::generate();
            let events = [unlocked, underpaid, payment]
                .into_iter()
                .map(|payment| {
                    PrivateDirectMessageBuilder::new(
                        info.pubkey,
                        serde_json::to_string(&payment).unwrap(),
                    )
                    .finalize(&sender)
                    .unwrap()
                })
                .collect();
            let relay_task = tokio::spawn(relay(listener, events));
            let barrier = Arc::new(tokio::sync::Barrier::new(2));
            *mock.post_swap_barrier.lock().unwrap() = Some(barrier.clone());
            let receiving_repository = repository.clone();
            let receiving_info = info.clone();
            let receive_task = tokio::spawn(async move {
                receiving_repository
                    .wait_for_nostr_payment_with_options(
                        receiving_info,
                        ReceiveOptions {
                            p2pk_signing_keys: vec![signing_key],
                            preimages: htlc.then_some(preimage).into_iter().collect(),
                            ..Default::default()
                        },
                    )
                    .await
            });
            let amount = timeout(Duration::from_secs(10), async {
                barrier.wait().await;
                // The mint now has the swap. Cancellation must not discard its response.
                info.cancel();
                barrier.wait().await;
                receive_task.await.unwrap()
            })
            .await
            .unwrap()
            .unwrap();
            assert_eq!(amount, Amount::from(4));
            assert_eq!(wallet.total_balance().await.unwrap(), amount);
            {
                let swaps = mock.captured_swap_requests.lock().unwrap();
                assert_eq!(swaps.len(), 1);
                let input = &swaps[0].inputs()[0];
                match htlc {
                    true => input.verify_htlc().unwrap(),
                    false => input.verify_p2pk().unwrap(),
                }
            }
            assert!(matches!(
                repository
                    .wait_for_nostr_payment_with_options(info, ReceiveOptions::default())
                    .await,
                Err(Error::PaymentRequestReceiveConsumed)
            ));
            timeout(Duration::from_secs(2), relay_task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}
