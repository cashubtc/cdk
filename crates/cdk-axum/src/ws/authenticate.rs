use std::str::FromStr;

use cdk::error::ErrorCode;
use cdk::nuts::nut21::{Method, ProtectedEndpoint, RoutePath};
use cdk::nuts::{AuthToken, BlindAuthToken};
use cdk::ws::{WsAuthenticateRequest, WsAuthenticateResponse, WsResponseResult};

use super::{WsContext, WsError};
use crate::MintState;

/// Handle a NUT-22 `authenticate` command.
///
/// Verifies and spends the blind authentication token, then marks the whole
/// connection authenticated for its lifetime. A single BAT authenticates the
/// connection; later commands do not consume additional tokens.
///
/// Every rejected token costs a full signature verification on a connection
/// nobody has paid for yet, so failures are counted and the caller closes the
/// connection once too many pile up.
pub(crate) async fn handle(
    context: &mut WsContext,
    req: WsAuthenticateRequest,
) -> Result<WsResponseResult, WsError> {
    // A single BAT authenticates the connection for its lifetime (NUT-22), so a
    // repeat authenticate is a no-op and must not spend another token.
    if context.authenticated {
        return Ok(WsAuthenticateResponse::Ok.into());
    }

    match verify(&context.state, req).await {
        Ok(()) => {
            context.authenticated = true;
            Ok(WsAuthenticateResponse::Ok.into())
        }
        Err(err) => {
            context.failed_auth_attempts += 1;
            Err(err)
        }
    }
}

async fn verify(state: &MintState, req: WsAuthenticateRequest) -> Result<(), WsError> {
    let token = BlindAuthToken::from_str(&req.token).map_err(|_| blind_auth_failed())?;

    state
        .mint
        .verify_auth(
            Some(AuthToken::BlindAuth(token)),
            &ProtectedEndpoint::new(Method::Get, RoutePath::Ws),
        )
        .await
        .map_err(|_| blind_auth_failed())
}

fn blind_auth_failed() -> WsError {
    WsError::ServerError(
        ErrorCode::BlindAuthFailed.to_code() as i32,
        "Blind authentication failed".to_string(),
    )
}
