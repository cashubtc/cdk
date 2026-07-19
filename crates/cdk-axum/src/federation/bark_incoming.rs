//! Consensus-first orchestration for federated Bark incoming BOLT11 quotes.

use std::time::{SystemTime, UNIX_EPOCH};

use cdk::federation::{
    ark_server_commitment, federation_bark_description_hash, federation_bark_mailbox_scope,
    payment_hash_for_preimage, preimage_commitment, FederationBarkActionId,
    FederationBarkOperation, FederationBarkReceiveIntent, FederationBarkReceivePrepared,
    FederationBarkReceiveProof, FederationBarkReceiveSecretBinding, FederationBarkUnit,
    FederationBarkVtxoLocator, FederationOperation, FederationOperationId, MemberConfigPublic,
    FEDERATION_BARK_OPERATION_VERSION, FEDERATION_MAX_DERIVATION_INDEX,
};
use cdk::mint::{MintQuoteRequest, MintQuoteResponse, QuoteId};
use cdk::nuts::CurrencyUnit;
use cdk::Amount;
use cdk_frost::rand_core::{OsRng, RngCore};
use cdk_http_client::Async;

use crate::MintState;

use super::bark_secret::FederationBarkReceiveSecretCoordinator;
use super::peer_client::FederationRestPeerClient;

const FEDERATION_BARK_RECEIVE_INTENT_LIFETIME_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Prepare the ordinary CDK quote from a consensus-accepted Bark invoice.
pub(crate) async fn prepare_federated_bark_mint_quote(
    state: &MintState,
    quote_id: QuoteId,
    request: MintQuoteRequest,
) -> Result<
    (
        MintQuoteResponse<QuoteId>,
        cdk::cdk_payment::PaymentIdentifier,
    ),
    cdk::Error,
> {
    let _guard = state.federation.bark_incoming_lock().lock().await;
    if let Some(quote) = state.mint.localstore().get_mint_quote(&quote_id).await? {
        let request_lookup_id = quote.request_lookup_id.clone();
        return Ok((quote.try_into()?, request_lookup_id));
    }
    let invoice = accepted_invoice(state, &quote_id, &request).await?;
    state
        .mint
        .prepare_bolt11_mint_quote_with_invoice(quote_id, request, invoice)
        .await
}

async fn accepted_invoice(
    state: &MintState,
    quote_id: &QuoteId,
    request: &MintQuoteRequest,
) -> Result<cdk::Bolt11Invoice, cdk::Error> {
    let MintQuoteRequest::Bolt11(bolt11) = request else {
        return Err(cdk::Error::InvalidPaymentRequest);
    };
    if bolt11.unit != CurrencyUnit::Sat
        || bolt11.amount == Amount::ZERO
        || federation_bark_description_hash(bolt11.description.as_deref()).is_err()
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    let limits = runtime.limits();
    let amount = u64::from(bolt11.amount);
    if amount < limits.minimum_sat() || amount > limits.maximum_sat() {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    let projection = state.mint.federation_bark_projection_snapshot().await?;
    let quote = quote_id.to_string();
    let action = projection
        .actions
        .iter()
        .find(|action| action.quote.as_deref() == Some(quote.as_str()));

    if let Some(action) = action {
        return match &action.operation {
            FederationBarkOperation::ReceivePrepared(prepared) => {
                let intent = state
                    .mint
                    .federation_bark_receive_intent(prepared.intent_operation_id)
                    .await?
                    .ok_or(cdk::Error::InvalidPaymentRequest)?;
                validate_request_against_intent(bolt11, &intent)?;
                prepared
                    .validate_against_intent(&intent)
                    .map_err(cdk::Error::from)?;
                validate_request_against_prepared(bolt11, prepared)?;
                prepared
                    .invoice
                    .parse()
                    .map_err(|_| cdk::Error::InvalidPaymentRequest)
            }
            FederationBarkOperation::ReceiveIntent(intent) => {
                validate_request_against_intent(bolt11, intent)?;
                prepare_and_accept_invoice(
                    state,
                    action.latest_operation_id,
                    action.latest_operation_index,
                    intent,
                )
                .await
            }
            FederationBarkOperation::ActionPlan(_)
            | FederationBarkOperation::ActionTransition(_)
            | FederationBarkOperation::MemberObservation(_) => {
                Err(cdk::Error::InvalidPaymentRequest)
            }
        };
    }

    let store = runtime.receive_secret_store();
    let resumable = store
        .find_by_quote(federation.federation_id(), &quote)
        .map_err(|_| bark_unavailable())?;
    let now = unix_time()?;
    let (mut intent, secret) = match resumable {
        Some(resumable) => {
            validate_binding_against_request(federation.member_id(), bolt11, &resumable.binding)?;
            (
                intent_from_binding(&resumable.binding, bolt11.description.clone(), Vec::new()),
                resumable.secret,
            )
        }
        None => fresh_intent(state, quote_id, bolt11, &projection, now).await?,
    };
    let binding =
        FederationBarkReceiveSecretBinding::from_intent(federation.federation_id(), &intent);
    let local_request = cdk::federation::FederationBarkPrepareReceiveSecretRequest::new(
        binding.clone(),
        federation.member_id(),
        *secret.as_bytes(),
    );
    let local_acknowledgement = store
        .prepare_receive_secret(
            &local_request,
            federation.member_id(),
            federation.public_config(),
            federation.private_config(),
            now,
        )
        .map_err(|_| bark_unavailable())?;
    let remote_members = federation
        .public_config()
        .members
        .values()
        .filter(|member| member.member_id != federation.member_id())
        .cloned()
        .collect::<Vec<MemberConfigPublic>>();
    let peer_client = FederationRestPeerClient::new_with_validated_config(
        Async::default(),
        federation.public_config(),
        federation.private_config().clone(),
        state
            .federation
            .peer_sync_config
            .ok_or_else(bark_unavailable)?,
    )
    .map_err(|_| bark_unavailable())?;
    intent.secret_acknowledgements = FederationBarkReceiveSecretCoordinator::new(&peer_client)
        .distribute(
            federation.public_config(),
            &binding,
            &secret,
            &remote_members,
            vec![local_acknowledgement],
        )
        .await
        .map_err(|_| bark_unavailable())?;
    intent
        .validate_secret_acknowledgements(federation.public_config())
        .map_err(cdk::Error::from)?;
    let envelope = federation.operation_envelope(FederationOperation::Bark(
        FederationBarkOperation::ReceiveIntent(intent.clone()),
    ));
    let accepted = state.submit_federation_envelope(envelope).await?;
    let entry = accepted.outcome.entry();
    store
        .mark_intent_accepted(
            intent.action_id,
            cdk_bark::FederatedBarkReceiveSecretAcceptedRef {
                operation_id: entry.operation_id,
                operation_index: entry.index,
            },
        )
        .map_err(|_| bark_unavailable())?;
    prepare_and_accept_invoice(state, entry.operation_id, entry.index, &intent).await
}

async fn fresh_intent(
    state: &MintState,
    quote_id: &QuoteId,
    request: &cdk::nuts::MintQuoteBolt11Request,
    projection: &cdk::federation::FederationBarkProjectionSnapshot,
    now: u64,
) -> Result<
    (
        FederationBarkReceiveIntent,
        cdk::federation::FederationBarkReceiveSecret,
    ),
    cdk::Error,
> {
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    let application = federation
        .public_config()
        .frost_wallet
        .as_ref()
        .and_then(|wallet| wallet.bark_application.as_ref())
        .ok_or_else(bark_unavailable)?;
    let next_index = projection
        .derivation_allocations
        .iter()
        .map(|allocation| allocation.locator.index)
        .max()
        .map_or(Ok(0), |index| {
            index.checked_add(1).ok_or_else(bark_unavailable)
        })?;
    if next_index > FEDERATION_MAX_DERIVATION_INDEX {
        return Err(bark_unavailable());
    }
    let mailbox_id = runtime
        .derive_public_key(next_index)
        .map_err(|_| bark_unavailable())?
        .serialize();
    let mut preimage = [0; 32];
    OsRng.fill_bytes(&mut preimage);
    let quote = quote_id.to_string();
    // The Ark server owns the exact invoice expiry (its default may be 48h).
    // This is only the consensus preparation deadline; the ordinary MintQuote
    // below retains the exact returned BOLT11 expiry.
    let expires_at = now
        .checked_add(FEDERATION_BARK_RECEIVE_INTENT_LIFETIME_SECONDS)
        .ok_or_else(bark_unavailable)?;
    let intent = FederationBarkReceiveIntent {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: FederationBarkActionId::derive_receive_intent(
            federation.federation_id(),
            &quote,
        )
        .map_err(cdk::Error::from)?,
        quote,
        actor: federation.member_id(),
        cashu_amount: request.amount,
        gross_amount: request.amount,
        receive_fee: Amount::ZERO,
        minimum_net_amount: request.amount,
        unit: FederationBarkUnit::Sat,
        description_hash: federation_bark_description_hash(request.description.as_deref())
            .map_err(cdk::Error::from)?,
        description: request.description.clone(),
        payment_hash: payment_hash_for_preimage(&preimage).map_err(cdk::Error::from)?,
        preimage_commitment: preimage_commitment(&preimage).map_err(cdk::Error::from)?,
        secret_acknowledgements: Vec::new(),
        destination: FederationBarkVtxoLocator::vtxo_custody(next_index),
        mailbox_scope: federation_bark_mailbox_scope(&mailbox_id).map_err(cdk::Error::from)?,
        mailbox_id,
        ark_server: ark_server_commitment(&application.ark_server_public_key)
            .map_err(cdk::Error::from)?,
        network: application.network,
        minimum_cltv_delta: application.policy.minimum_incoming_cltv_delta,
        fee_policy_version: 1,
        anti_dos_policy_version: 1,
        receive_proof: FederationBarkReceiveProof::NoProof,
        expires_at,
    };
    Ok((
        intent,
        cdk::federation::FederationBarkReceiveSecret::from_bytes(preimage),
    ))
}

async fn prepare_and_accept_invoice(
    state: &MintState,
    intent_operation_id: FederationOperationId,
    intent_operation_index: u64,
    intent: &FederationBarkReceiveIntent,
) -> Result<cdk::Bolt11Invoice, cdk::Error> {
    let federation = state.mint.federation_config().ok_or(cdk::Error::Internal)?;
    if intent.actor != federation.member_id() {
        return Err(bark_unavailable());
    }
    let runtime = state
        .federation
        .bark_runtime()
        .ok_or_else(bark_unavailable)?;
    runtime
        .receive_secret_store()
        .mark_intent_accepted(
            intent.action_id,
            cdk_bark::FederatedBarkReceiveSecretAcceptedRef {
                operation_id: intent_operation_id,
                operation_index: intent_operation_index,
            },
        )
        .map_err(|_| bark_unavailable())?;
    let prepared = runtime
        .prepare_receive_invoice(intent_operation_id, intent)
        .await
        .map_err(|_| bark_unavailable())?;
    let envelope = federation.operation_envelope(FederationOperation::Bark(
        FederationBarkOperation::ReceivePrepared(prepared.clone()),
    ));
    state.submit_federation_envelope(envelope).await?;
    if runtime
        .apply_receive_prepared(intent, &prepared)
        .await
        .is_err()
    {
        runtime
            .mark_degraded(cdk_bark::FederatedBarkDegradedReason::ProjectionInvalid)
            .await;
        tracing::error!(
            action_id = %intent.action_id,
            "accepted Bark receive plan could not be materialized locally"
        );
    }
    prepared
        .invoice
        .parse()
        .map_err(|_| cdk::Error::InvalidPaymentRequest)
}

fn intent_from_binding(
    binding: &FederationBarkReceiveSecretBinding,
    description: Option<String>,
    secret_acknowledgements: Vec<cdk::federation::FederationBarkReceiveSecretAcknowledgement>,
) -> FederationBarkReceiveIntent {
    FederationBarkReceiveIntent {
        version: FEDERATION_BARK_OPERATION_VERSION,
        action_id: binding.action_id,
        quote: binding.quote.clone(),
        actor: binding.actor,
        cashu_amount: binding.cashu_amount,
        gross_amount: binding.gross_amount,
        receive_fee: binding.receive_fee,
        minimum_net_amount: binding.minimum_net_amount,
        unit: binding.unit,
        description_hash: binding.description_hash,
        description,
        payment_hash: binding.payment_hash,
        preimage_commitment: binding.preimage_commitment,
        secret_acknowledgements,
        destination: binding.destination,
        mailbox_scope: binding.mailbox_scope,
        mailbox_id: binding.mailbox_id,
        ark_server: binding.ark_server,
        network: binding.network,
        minimum_cltv_delta: binding.minimum_cltv_delta,
        fee_policy_version: binding.fee_policy_version,
        anti_dos_policy_version: binding.anti_dos_policy_version,
        receive_proof: binding.receive_proof,
        expires_at: binding.expires_at,
    }
}

fn validate_binding_against_request(
    member_id: cdk::federation::MemberId,
    request: &cdk::nuts::MintQuoteBolt11Request,
    binding: &FederationBarkReceiveSecretBinding,
) -> Result<(), cdk::Error> {
    if binding.actor != member_id
        || binding.cashu_amount != request.amount
        || binding.gross_amount != request.amount
        || binding.description_hash
            != federation_bark_description_hash(request.description.as_deref())
                .map_err(cdk::Error::from)?
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    Ok(())
}

fn validate_request_against_intent(
    request: &cdk::nuts::MintQuoteBolt11Request,
    intent: &FederationBarkReceiveIntent,
) -> Result<(), cdk::Error> {
    if intent.cashu_amount != request.amount
        || intent.unit != FederationBarkUnit::Sat
        || intent.description != request.description
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    Ok(())
}

fn validate_request_against_prepared(
    request: &cdk::nuts::MintQuoteBolt11Request,
    prepared: &FederationBarkReceivePrepared,
) -> Result<(), cdk::Error> {
    let invoice = prepared
        .invoice
        .parse::<cdk::Bolt11Invoice>()
        .map_err(|_| cdk::Error::InvalidPaymentRequest)?;
    let expected_msat = u64::from(request.amount)
        .checked_mul(1_000)
        .ok_or(cdk::Error::InvalidPaymentRequest)?;
    if prepared.gross_amount != request.amount
        || invoice.amount_milli_satoshis() != Some(expected_msat)
    {
        return Err(cdk::Error::InvalidPaymentRequest);
    }
    Ok(())
}

fn unix_time() -> Result<u64, cdk::Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| bark_unavailable())
}

fn bark_unavailable() -> cdk::Error {
    cdk::Error::Custom("federated Bark incoming backend is unavailable".to_string())
}
