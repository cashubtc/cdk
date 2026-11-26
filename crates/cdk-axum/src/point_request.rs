//! Admit mint request arrays before decoding their protocol points.

use std::collections::HashSet;
use std::sync::LazyLock;

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Json, Request};
use axum::response::{IntoResponse, Response};
use cdk::nuts::{Id, KeySetVersion};
use cdk_common::work_pool::{Admission, WorkPool};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::{into_response, MintState};

/// Decodes v3 request bodies off the async workers. Decoding precedes any
/// state change, so a request that finds the pool full is refused.
static POINT_DECODERS: LazyLock<WorkPool> = LazyLock::new(WorkPool::sized_to_host);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointBatch {
    Spend,
    Outputs,
    /// Restore queries identify historical signatures by blinded point,
    /// including when the signatory no longer has the corresponding keyset.
    Restore,
}

/// A JSON body carrying blinded outputs, optionally alongside proof inputs.
pub(crate) struct PointJson<T, const INPUTS: bool = false>(pub T);

impl<T, const INPUTS: bool> FromRequest<MintState> for PointJson<T, INPUTS>
where
    T: DeserializeOwned + Send + 'static,
{
    type Rejection = Response;

    async fn from_request(req: Request, state: &MintState) -> Result<Self, Self::Rejection> {
        let Json(raw) = Json::<Box<RawValue>>::from_request(req, state)
            .await
            .map_err(IntoResponse::into_response)?;
        let batch = if INPUTS {
            PointBatch::Spend
        } else {
            PointBatch::Outputs
        };
        decode(state, raw, batch, IntoResponse::into_response)
            .await
            .map(Self)
    }
}

#[derive(Deserialize)]
struct KeysetRef {
    id: Id,
}

#[derive(Deserialize)]
struct OutputRefs {
    outputs: Option<Vec<KeysetRef>>,
}

#[derive(Deserialize)]
struct InputOutputRefs {
    inputs: Vec<KeysetRef>,
    outputs: Option<Vec<KeysetRef>>,
}

/// Decode using the same bytes inspected at admission, preserving Serde's
/// duplicate-field checks and Axum's normal JSON rejection responses.
pub(crate) async fn decode<T>(
    state: &MintState,
    raw: Box<RawValue>,
    batch: PointBatch,
    json_error: fn(JsonRejection) -> Response,
) -> Result<T, Response>
where
    T: DeserializeOwned + Send + 'static,
{
    let bytes = raw.get().as_bytes();
    let (inputs, outputs) = if batch == PointBatch::Spend {
        let Json(refs) = Json::<InputOutputRefs>::from_bytes(bytes).map_err(json_error)?;
        (refs.inputs, refs.outputs)
    } else {
        let Json(refs) = Json::<OutputRefs>::from_bytes(bytes).map_err(json_error)?;
        (Vec::new(), refs.outputs)
    };
    let outputs = outputs.unwrap_or_default();
    if inputs.len() > state.mint.max_inputs() {
        return Err(into_response(cdk::Error::MaxInputsExceeded {
            actual: inputs.len(),
            max: state.mint.max_inputs(),
        }));
    }
    if outputs.len() > state.mint.max_outputs() {
        return Err(into_response(cdk::Error::MaxOutputsExceeded {
            actual: outputs.len(),
            max: state.mint.max_outputs(),
        }));
    }

    let bls_keysets: HashSet<_> = inputs
        .into_iter()
        .chain(outputs)
        .map(|item| item.id)
        .filter(|id| id.get_version() == KeySetVersion::Version02)
        .collect();
    let decode_body = move || {
        Json::<T>::from_bytes(raw.get().as_bytes())
            .map(|json| json.0)
            .map_err(json_error)
    };
    if bls_keysets.is_empty() {
        return decode_body();
    }
    for id in bls_keysets {
        if batch != PointBatch::Restore && state.mint.get_keyset_info(&id).is_none() {
            return Err(into_response(cdk::Error::UnknownKeySet));
        }
    }

    POINT_DECODERS
        .run(Admission::Bounded, decode_body)
        .await
        .map_err(into_response)?
}
