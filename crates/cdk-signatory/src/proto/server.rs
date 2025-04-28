use std::net::SocketAddr;

use tonic::transport::{Error, Server};
use tonic::{Request, Response, Status};

use super::{boolean_response, key_rotation_response, keys_response, BooleanResponse};
use crate::proto::{self, signatory_server};
use crate::signatory::Signatory;

pub struct CdkSignatoryServer<T>
where
    T: Signatory + Send + Sync + 'static,
{
    inner: T,
}

#[tonic::async_trait]
impl<T> signatory_server::Signatory for CdkSignatoryServer<T>
where
    T: Signatory + Send + Sync + 'static,
{
    async fn blind_sign(
        &self,
        request: Request<proto::BlindedMessages>,
    ) -> Result<Response<proto::BlindSignResponse>, Status> {
        let result = match self
            .inner
            .blind_sign(
                request
                    .into_inner()
                    .blinded_messages
                    .into_iter()
                    .map(|blind_message| blind_message.try_into())
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .await
        {
            Ok(blind_signatures) => {
                proto::blind_sign_response::Result::Sigs(proto::BlindSignatures {
                    blind_signatures: blind_signatures
                        .into_iter()
                        .map(|blind_sign| blind_sign.into())
                        .collect(),
                })
            }
            Err(err) => proto::blind_sign_response::Result::Error(err.into()),
        };

        //.map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(Response::new(proto::BlindSignResponse {
            result: Some(result),
        }))
    }

    async fn verify_proofs(
        &self,
        request: Request<proto::Proofs>,
    ) -> Result<Response<proto::BooleanResponse>, Status> {
        let result = match self
            .inner
            .verify_proofs(
                request
                    .into_inner()
                    .proof
                    .into_iter()
                    .map(|x| x.try_into())
                    .collect::<Result<Vec<_>, _>>()?,
            )
            .await
        {
            Ok(()) => boolean_response::Result::Success(true),

            Err(cdk_common::Error::DHKE(_)) => boolean_response::Result::Success(false),
            Err(err) => boolean_response::Result::Error(err.into()),
        };

        Ok(Response::new(BooleanResponse {
            result: Some(result),
        }))
    }

    async fn keysets(
        &self,
        _request: Request<proto::EmptyRequest>,
    ) -> Result<Response<proto::KeysResponse>, Status> {
        let result = match self.inner.keysets().await {
            Ok(result) => keys_response::Result::Keysets(result.into()),
            Err(err) => keys_response::Result::Error(err.into()),
        };

        Ok(Response::new(proto::KeysResponse {
            result: Some(result),
        }))
    }

    async fn rotate_keyset(
        &self,
        request: Request<proto::RotationRequest>,
    ) -> Result<Response<proto::KeyRotationResponse>, Status> {
        let mint_keyset_info = match self
            .inner
            .rotate_keyset(request.into_inner().try_into()?)
            .await
        {
            Ok(result) => key_rotation_response::Result::Keyset(result.into()),
            Err(err) => key_rotation_response::Result::Error(err.into()),
        };

        Ok(Response::new(proto::KeyRotationResponse {
            result: Some(mint_keyset_info),
        }))
    }
}

/// Runs the signatory server
pub async fn grpc_server<T>(signatory: T, addr: SocketAddr) -> Result<(), Error>
where
    T: Signatory + Send + Sync + 'static,
{
    tracing::info!("grpc_server listening on {}", addr);
    Server::builder()
        .add_service(signatory_server::SignatoryServer::new(CdkSignatoryServer {
            inner: signatory,
        }))
        .serve(addr)
        .await?;
    Ok(())
}
