//! LDK Node Errors

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {}

impl From<Error> for cdk_common::payment::Error {
    fn from(e: Error) -> Self {
        Self::Lightning(Box::new(e))
    }
}
