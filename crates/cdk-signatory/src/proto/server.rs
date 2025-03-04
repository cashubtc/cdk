use std::net::SocketAddr;

use cdk_common::dhke;
use cdk_common::signatory::Signatory as _;
use tonic::transport::{Error, Server};
use tonic::{Request, Response, Status};

use crate::proto::{self, signatory_server};
use crate::MemorySignatory;

struct CdkSignatory(MemorySignatory);

#[tonic::async_trait]
impl signatory_server::Signatory for CdkSignatory {
    async fn blind_sign(
        &self,
        request: Request<proto::BlindedMessage>,
    ) -> Result<Response<proto::BlindSignature>, Status> {
        println!("Got a request: {:?}", request);
        let blind_signature = self
            .0
            .blind_sign(request.into_inner().try_into()?)
            .await
            .map_err(|e| Status::from_error(Box::new(e)))?;
        Ok(Response::new(blind_signature.into()))
    }

    async fn verify_proof(
        &self,
        request: Request<proto::Proof>,
    ) -> Result<Response<proto::Success>, Status> {
        println!("Got a request: {:?}", request);
        let result = match self.0.verify_proof(request.into_inner().try_into()?).await {
            Ok(()) => proto::Success { success: true },
            Err(cdk_common::error::Error::DHKE(dhke::Error::TokenNotVerified)) => {
                proto::Success { success: false }
            }
            Err(err) => return Err(Status::from_error(Box::new(err))),
        };

        Ok(Response::new(result))
    }
}

/// Runs the signatory server
pub async fn grpc_server(signatory: MemorySignatory, addr: SocketAddr) -> Result<(), Error> {
    tracing::info!("grpc_server listening on {}", addr);
    Server::builder()
        .add_service(signatory_server::SignatoryServer::new(CdkSignatory(
            signatory,
        )))
        .serve(addr)
        .await?;
    Ok(())
}
