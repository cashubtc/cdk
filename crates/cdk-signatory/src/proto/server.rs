use std::net::SocketAddr;

use tonic::transport::{Error, Server};
use tonic::{Request, Response, Status};

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
        request: Request<proto::BlindedMessage>,
    ) -> Result<Response<proto::BlindSignature>, Status> {
        let blind_signature = self
            .inner
            .blind_sign(request.into_inner().try_into()?)
            .await
            .map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(Response::new(blind_signature.into()))
    }

    async fn verify_proof(
        &self,
        request: Request<proto::Proof>,
    ) -> Result<Response<proto::Empty>, Status> {
        self.inner
            .verify_proof(request.into_inner().try_into()?)
            .await
            .map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(Response::new(proto::Empty {}))
    }

    async fn auth_keysets(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::VecSignatoryKeySet>, Status> {
        let keys_response = self
            .inner
            .auth_keysets()
            .await
            .map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(Response::new(if let Some(keys_response) = keys_response {
            proto::VecSignatoryKeySet {
                keysets: keys_response.into_iter().map(|k| k.into()).collect(),
                is_none: Some(false),
            }
        } else {
            proto::VecSignatoryKeySet {
                keysets: vec![],
                is_none: Some(true),
            }
        }))
    }

    async fn keysets(
        &self,
        _request: Request<proto::Empty>,
    ) -> Result<Response<proto::VecSignatoryKeySet>, Status> {
        let keys_response = self
            .inner
            .keysets()
            .await
            .map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(Response::new(proto::VecSignatoryKeySet {
            keysets: keys_response.into_iter().map(|k| k.into()).collect(),
            is_none: Some(false),
        }))
    }

    async fn rotate_keyset(
        &self,
        request: Request<proto::RotateKeyArguments>,
    ) -> Result<Response<proto::MintKeySetInfo>, Status> {
        let mint_keyset_info = self
            .inner
            .rotate_keyset(request.into_inner().try_into()?)
            .await
            .map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(Response::new(mint_keyset_info.into()))
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
