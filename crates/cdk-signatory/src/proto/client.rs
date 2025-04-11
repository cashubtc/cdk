use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::{BlindSignature, BlindedMessage, Proof};

use crate::proto::signatory_client::SignatoryClient;
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet};

/// A client for the Signatory service.
pub struct SignatoryRpcClient {
    client: SignatoryClient<tonic::transport::Channel>,
}

impl SignatoryRpcClient {
    /// Create a new RemoteSigner from a tonic transport channel.
    pub async fn new(url: String) -> Result<Self, tonic::transport::Error> {
        Ok(Self {
            client: SignatoryClient::connect(url).await?,
        })
    }
}

#[async_trait::async_trait]
impl Signatory for SignatoryRpcClient {
    async fn blind_sign(&self, request: BlindedMessage) -> Result<BlindSignature, Error> {
        let req: super::BlindedMessage = request.into();
        self.client
            .clone()
            .blind_sign(req)
            .await
            .map(|response| response.into_inner().try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn verify_proof(&self, proof: Proof) -> Result<(), Error> {
        let req: super::Proof = proof.into();
        self.client
            .clone()
            .verify_proof(req)
            .await
            .map(|response| response.into_inner().try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn auth_keysets(&self) -> Result<Option<Vec<SignatoryKeySet>>, Error> {
        self.client
            .clone()
            .auth_keysets(super::Empty {})
            .await
            .map(|response| {
                let response = response.into_inner();

                if response.is_none == Some(true) {
                    Ok(None)
                } else {
                    response
                        .keysets
                        .into_iter()
                        .map(|x| x.try_into())
                        .collect::<Result<Vec<SignatoryKeySet>, _>>()
                        .map(Some)
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn keysets(&self) -> Result<Vec<SignatoryKeySet>, Error> {
        self.client
            .clone()
            .keysets(super::Empty {})
            .await
            .map(|response| {
                response
                    .into_inner()
                    .keysets
                    .into_iter()
                    .map(|x| x.try_into())
                    .collect::<Result<Vec<SignatoryKeySet>, _>>()
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<MintKeySetInfo, Error> {
        let req: super::RotateKeyArguments = args.into();
        self.client
            .clone()
            .rotate_keyset(req)
            .await
            .map(|response| response.into_inner().try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }
}
