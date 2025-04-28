use cdk_common::error::Error;
use cdk_common::{BlindSignature, BlindedMessage, Proof};

use super::{blind_sign_response, boolean_response, key_rotation_response, keys_response};
use crate::proto::signatory_client::SignatoryClient;
use crate::signatory::{RotateKeyArguments, Signatory, SignatoryKeySet, SignatoryKeysets};

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
    async fn blind_sign(&self, request: Vec<BlindedMessage>) -> Result<Vec<BlindSignature>, Error> {
        let req = super::BlindedMessages {
            blinded_messages: request
                .into_iter()
                .map(|blind_message| blind_message.into())
                .collect(),
        };

        self.client
            .clone()
            .blind_sign(req)
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    blind_sign_response::Result::Sigs(sigs) => sigs
                        .blind_signatures
                        .into_iter()
                        .map(|blinded_signature| blinded_signature.try_into())
                        .collect(),
                    blind_sign_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn verify_proofs(&self, proofs: Vec<Proof>) -> Result<(), Error> {
        let req: super::Proofs = proofs.into();
        self.client
            .clone()
            .verify_proofs(req)
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    boolean_response::Result::Success(bool) => {
                        if bool {
                            Ok(())
                        } else {
                            Err(Error::SignatureMissingOrInvalid)
                        }
                    }
                    boolean_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn keysets(&self) -> Result<SignatoryKeysets, Error> {
        self.client
            .clone()
            .keysets(super::EmptyRequest {})
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    keys_response::Result::Keysets(keyset) => keyset.try_into(),
                    keys_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn rotate_keyset(&self, args: RotateKeyArguments) -> Result<SignatoryKeySet, Error> {
        let req: super::RotationRequest = args.into();
        self.client
            .clone()
            .rotate_keyset(req)
            .await
            .map(|response| {
                match response
                    .into_inner()
                    .result
                    .ok_or(Error::Custom("Internal error".to_owned()))?
                {
                    key_rotation_response::Result::Keyset(keyset) => keyset.try_into(),
                    key_rotation_response::Result::Error(err) => Err(err.into()),
                }
            })
            .map_err(|e| Error::Custom(e.to_string()))?
    }
}
