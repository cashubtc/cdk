use std::collections::HashMap;

use bitcoin::bip32::DerivationPath;
use cdk_common::error::Error;
use cdk_common::mint::MintKeySetInfo;
use cdk_common::signatory::{KeysetIdentifier, Signatory};
use cdk_common::{
    BlindSignature, BlindedMessage, CurrencyUnit, Id, KeySet, KeysResponse, KeysetResponse, Proof,
};

use crate::proto::signatory_client::SignatoryClient;

/// A client for the Signatory service.
pub struct RemoteSigner {
    client: SignatoryClient<tonic::transport::Channel>,
}

impl RemoteSigner {
    /// Create a new RemoteSigner from a tonic transport channel.
    pub async fn new(url: String) -> Result<Self, tonic::transport::Error> {
        Ok(Self {
            client: SignatoryClient::connect(url).await?,
        })
    }
}

#[async_trait::async_trait]
impl Signatory for RemoteSigner {
    async fn blind_sign(&self, request: BlindedMessage) -> Result<BlindSignature, Error> {
        let req: super::BlindedMessage = request.into();
        self.client
            .clone()
            .blind_sign(req)
            .await
            .map(|response| response.into_inner().try_into())
            .map_err(|e| Error::Custom(e.to_string()))?
    }

    async fn verify_proof(&self, _proof: Proof) -> Result<(), Error> {
        todo!()
    }
    async fn keyset(&self, _keyset_id: Id) -> Result<Option<KeySet>, Error> {
        todo!()
    }

    async fn keyset_pubkeys(&self, _keyset_id: Id) -> Result<KeysResponse, Error> {
        todo!()
    }

    async fn pubkeys(&self) -> Result<KeysResponse, Error> {
        todo!()
    }

    async fn keysets(&self) -> Result<KeysetResponse, Error> {
        todo!()
    }

    async fn get_keyset_info(&self, _keyset_id: KeysetIdentifier) -> Result<MintKeySetInfo, Error> {
        todo!()
    }

    async fn rotate_keyset(
        &self,
        _unit: CurrencyUnit,
        _derivation_path_index: u32,
        _max_order: u8,
        _input_fee_ppk: u64,
        _custom_paths: HashMap<CurrencyUnit, DerivationPath>,
    ) -> Result<MintKeySetInfo, Error> {
        todo!()
    }
}
