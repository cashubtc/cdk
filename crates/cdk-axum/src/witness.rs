//! `POST /witness/add-checkpoint` — the inbound side of the
//! [C2SP tlog-witness protocol][spec], letting this mint cosign *other*
//! transparency logs' checkpoints (see `docs/adr/nut-xx.md`'s
//! Witnessing section, and [`cdk::mint::witness::Witness`]).
//!
//! [spec]: https://github.com/C2SP/C2SP/blob/main/tlog-witness.md

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use cdk::mint::witness::AddCheckpointError;
use cdk_tlog::witness::{ParseError, WitnessError};

use crate::MintState;

/// `POST /witness/add-checkpoint`
///
/// Status codes follow the spec exactly: 200 with cosignature lines on
/// success, 404/403/400/409/422 for the specific rejection reasons it
/// defines, and — per the race condition called out in the spec — the
/// witness persists its new state before responding, never after.
pub(crate) async fn post_add_checkpoint(State(state): State<MintState>, body: String) -> Response {
    let Some(witness) = state.mint.witness() else {
        return (StatusCode::NOT_FOUND, "this mint does not run a witness").into_response();
    };

    match witness.handle_add_checkpoint(&body).await {
        Ok(cosignature) => (StatusCode::OK, cosignature.to_line()).into_response(),
        Err(AddCheckpointError::Malformed(err)) => {
            let status = match err {
                ParseError::Checkpoint(_) => StatusCode::BAD_REQUEST,
                _ => StatusCode::BAD_REQUEST,
            };
            (status, err.to_string()).into_response()
        }
        Err(AddCheckpointError::Declined(err)) => match err {
            WitnessError::UnknownOrigin => (StatusCode::NOT_FOUND, err.to_string()).into_response(),
            WitnessError::NoTrustedSignature => {
                (StatusCode::FORBIDDEN, err.to_string()).into_response()
            }
            WitnessError::OldSizeExceedsCheckpoint => {
                (StatusCode::BAD_REQUEST, err.to_string()).into_response()
            }
            WitnessError::SizeConflict { stored, .. } => (
                StatusCode::CONFLICT,
                axum::response::AppendHeaders([("Content-Type", "text/x.tlog.size")]),
                format!("{stored}\n"),
            )
                .into_response(),
            WitnessError::RootHashConflict => {
                (StatusCode::CONFLICT, err.to_string()).into_response()
            }
            WitnessError::InvalidConsistencyProof => {
                (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()).into_response()
            }
        },
        Err(AddCheckpointError::Storage(err)) => {
            tracing::error!("witness storage error: {err}");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
        }
    }
}
