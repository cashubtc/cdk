//! Private federation error statuses and redaction.

use super::*;

#[test]
fn private_federation_error_statuses_preserve_failure_class() {
    assert_eq!(
        federation_error_status(&FederationError::RuntimeUnavailable {
            component: "consensus",
        }),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        federation_error_status(&FederationError::PendingConsensusMempoolFull {
            pending: 4,
            max: 4,
        }),
        axum::http::StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        federation_error_status(&FederationError::ConflictingOperation {
            conflict_key: "quote:test".to_string(),
            existing_operation_id: "existing".to_string(),
            attempted_operation_id: "attempted".to_string(),
        }),
        axum::http::StatusCode::CONFLICT
    );
    assert_eq!(
        federation_error_status(&FederationError::AlephBft(
            FederationAlephBftError::RecoveryMismatch {
                reason: "stored state mismatch"
            }
        )),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        federation_error_status(&FederationError::InvalidMemberId(0)),
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        federation_error_status(&FederationError::JournalConsensusEvidenceMissing { index: 1 }),
        axum::http::StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn private_internal_federation_error_hides_diagnostic_detail() {
    for error in [
        FederationError::AlephBft(FederationAlephBftError::RecoveryMismatch {
            reason: "secret=internal-value",
        }),
        FederationError::BackendFailure {
            backend: "Bark",
            phase: "observe outgoing payment",
            operation_id: Some(FederationOperationId::from_bytes([42; 32])),
            reason: "internal diagnostic sentinel".to_string(),
        },
    ] {
        let response = federation_error_response(error);
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            response_error_detail(response).await,
            "federation internal error"
        );
    }
}
