//! NUT-16: Animated QR Codes
//!
//! <https://github.com/cashubtc/nuts/blob/main/16.md>

use super::Token;
pub use bc_ur::{MultipartDecoder, MultipartEncoder, URCodable, URDecodable, UREncodable};
use dcbor::{
    CBORTagged, CBORTaggedDecodable, CBORTaggedEncodable, Result as CBORResult, Tag, CBOR,
};

impl CBORTaggedEncodable for Token {
    fn untagged_cbor(&self) -> CBOR {
        match self {
            Token::TokenV4(token_v4) => CBOR::try_from_data(
                token_v4
                    .to_raw_bytes()
                    .expect("Failed to convert TokenV4 to raw bytes"),
            )
            .expect("Failed to create CBOR from TokenV4 raw bytes"),
            Token::TokenV3(_) => {
                // Only TokenV4 is supported for CBOR encoding
                todo!("CBOR encoding for TokenV3 is not supported")
            }
        }
    }
}

impl CBORTaggedDecodable for Token {
    fn from_untagged_cbor(cbor: CBOR) -> CBORResult<Self>
    where
        Self: Sized,
    {
        cbor.try_into()
    }
}

impl CBORTagged for Token {
    fn cbor_tags() -> Vec<Tag> {
        vec![Tag::with_static_name(60000, "cashu")]
    }
}

impl TryFrom<CBOR> for Token {
    type Error = dcbor::Error;

    fn try_from(cbor: CBOR) -> dcbor::Result<Self> {
        let bytes = cbor.try_into_byte_string()?;
        let token_v4 = Token::TokenV4(
            super::TokenV4::try_from(&bytes)
                .map_err(|_| dcbor::Error::Custom("TokenV4".to_string()))?,
        );
        Ok(token_v4)
    }
}
