//! `/v1/audit/*` handlers for a mint's transparency log (NUT-XX draft; see
//! `docs/adr/0001-append-only-transparency-log.md`).
//!
//! All of these are read-only and simply proxy to
//! [`cdk::mint::transparency::TransparencyLogService`]. If the mint has no
//! transparency log attached (see [`cdk::mint::Mint::set_transparency_log`]),
//! every endpoint here returns 404.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::MintState;

fn not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        "this mint does not have a transparency log enabled",
    )
        .into_response()
}

fn internal_error(err: impl std::fmt::Display) -> Response {
    tracing::error!("transparency log audit endpoint error: {err}");
    (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
}

/// `GET /v1/audit/pubkey`
pub(crate) async fn get_pubkey(State(state): State<MintState>) -> Result<Response, Response> {
    let Some(service) = state.mint.transparency_log() else {
        return Err(not_configured());
    };

    #[derive(Serialize)]
    struct PubkeyResponse {
        origin: String,
        pubkey: String,
        signature_scheme: &'static str,
    }

    Ok(Json(PubkeyResponse {
        origin: service.origin().to_string(),
        pubkey: service.public_key_base64(),
        signature_scheme: "ed25519",
    })
    .into_response())
}

#[derive(Serialize)]
struct CheckpointResponse {
    checkpoint: String,
}

/// `GET /v1/audit/checkpoint`
pub(crate) async fn get_latest_checkpoint(
    State(state): State<MintState>,
) -> Result<Response, Response> {
    let Some(service) = state.mint.transparency_log() else {
        return Err(not_configured());
    };
    match service.latest_checkpoint().await {
        Ok(Some(checkpoint)) => Ok(Json(CheckpointResponse { checkpoint }).into_response()),
        Ok(None) => Err((StatusCode::NOT_FOUND, "log is empty").into_response()),
        Err(err) => Err(internal_error(err)),
    }
}

/// `GET /v1/audit/checkpoint/{tree_size}`
pub(crate) async fn get_checkpoint_at(
    State(state): State<MintState>,
    axum::extract::Path(tree_size): axum::extract::Path<u64>,
) -> Result<Response, Response> {
    let Some(service) = state.mint.transparency_log() else {
        return Err(not_configured());
    };
    match service.checkpoint_at(tree_size).await {
        Ok(Some(checkpoint)) => Ok(Json(CheckpointResponse { checkpoint }).into_response()),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            format!("no checkpoint at tree_size={tree_size}"),
        )
            .into_response()),
        Err(err) => Err(internal_error(err)),
    }
}

#[derive(Deserialize)]
pub(crate) struct EntriesQuery {
    start: u64,
    end: u64,
}

#[derive(Serialize)]
struct EntryResponse {
    seq: u64,
    entity_type: String,
    op: String,
    entity_id: String,
    payload: serde_json::Value,
    created_time: u64,
    leaf_hash: String,
}

#[derive(Serialize)]
struct EntriesResponse {
    start: u64,
    end: u64,
    entries: Vec<EntryResponse>,
}

/// `GET /v1/audit/entries?start=&end=`
pub(crate) async fn get_entries(
    State(state): State<MintState>,
    Query(query): Query<EntriesQuery>,
) -> Result<Response, Response> {
    const MAX_ENTRIES: u64 = 1000;

    let Some(service) = state.mint.transparency_log() else {
        return Err(not_configured());
    };
    if query.end <= query.start {
        return Ok(Json(EntriesResponse {
            start: query.start,
            end: query.start,
            entries: vec![],
        })
        .into_response());
    }

    let end = query.end.min(query.start + MAX_ENTRIES);
    let entries = service
        .entries(query.start, end)
        .await
        .map_err(internal_error)?;

    let actual_end = entries.last().map(|e| e.seq + 1).unwrap_or(query.start);
    let entries = entries
        .into_iter()
        .map(|e| EntryResponse {
            seq: e.seq,
            entity_type: e.entity_type.as_str().to_string(),
            op: match e.op {
                cdk::cdk_database::EventOp::Update => "update".to_string(),
                cdk::cdk_database::EventOp::Delete => "delete".to_string(),
            },
            entity_id: e.entity_id,
            payload: serde_json::from_slice(&e.payload).unwrap_or(serde_json::Value::Null),
            created_time: e.created_time,
            leaf_hash: hex::encode(e.leaf_hash),
        })
        .collect();

    Ok(Json(EntriesResponse {
        start: query.start,
        end: actual_end,
        entries,
    })
    .into_response())
}

#[derive(Deserialize)]
pub(crate) struct InclusionQuery {
    seq: u64,
    tree_size: u64,
}

#[derive(Serialize)]
struct InclusionResponse {
    seq: u64,
    tree_size: u64,
    leaf_hash: String,
    proof: Vec<String>,
}

/// `GET /v1/audit/proof/inclusion?seq=&tree_size=`
pub(crate) async fn get_inclusion_proof(
    State(state): State<MintState>,
    Query(query): Query<InclusionQuery>,
) -> Result<Response, Response> {
    let Some(service) = state.mint.transparency_log() else {
        return Err(not_configured());
    };
    if query.seq >= query.tree_size {
        return Err((StatusCode::BAD_REQUEST, "seq out of range").into_response());
    }

    let (leaf, proof) = service
        .inclusion_proof(query.seq, query.tree_size)
        .await
        .map_err(internal_error)?;

    Ok(Json(InclusionResponse {
        seq: query.seq,
        tree_size: query.tree_size,
        leaf_hash: hex::encode(leaf),
        proof: proof.into_iter().map(hex::encode).collect(),
    })
    .into_response())
}

#[derive(Deserialize)]
pub(crate) struct ConsistencyQuery {
    first: u64,
    second: u64,
}

#[derive(Serialize)]
struct ConsistencyResponse {
    first: u64,
    second: u64,
    proof: Vec<String>,
}

/// `GET /v1/audit/proof/consistency?first=&second=`
pub(crate) async fn get_consistency_proof(
    State(state): State<MintState>,
    Query(query): Query<ConsistencyQuery>,
) -> Result<Response, Response> {
    let Some(service) = state.mint.transparency_log() else {
        return Err(not_configured());
    };
    if query.first > query.second {
        return Err((StatusCode::BAD_REQUEST, "first must not exceed second").into_response());
    }

    let proof = service
        .consistency_proof(query.first, query.second)
        .await
        .map_err(internal_error)?;

    Ok(Json(ConsistencyResponse {
        first: query.first,
        second: query.second,
        proof: proof.into_iter().map(hex::encode).collect(),
    })
    .into_response())
}
