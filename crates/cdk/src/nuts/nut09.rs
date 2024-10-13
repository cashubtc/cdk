//! NUT-09: Restore signatures
//!
//! <https://github.com/cashubtc/nuts/blob/main/09.md>

use serde::{Deserialize, Serialize};

use super::nut00::{BlindSignature, BlindedMessage};

/// Restore Request [NUT-09]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreRequest {
    /// Outputs
    pub outputs: Vec<BlindedMessage>,
}

/// Restore Response [NUT-09]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResponse {
    /// Outputs
    pub outputs: Vec<BlindedMessage>,
    /// Signatures
    pub signatures: Vec<BlindSignature>,
}

mod test {

    #[test]
    fn restore_response() {
        use super::*;
        let rs = r#"{"outputs":[{"B_":"0204bbffa045f28ec836117a29ea0a00d77f1d692e38cf94f72a5145bfda6d8f41","amount":0,"id":"00ffd48b8f5ecf80", "witness":null},{"B_":"025f0615ccba96f810582a6885ffdb04bd57c96dbc590f5aa560447b31258988d7","amount":0,"id":"00ffd48b8f5ecf80"}],"signatures":[{"C_":"02e9701b804dc05a5294b5a580b428237a27c7ee1690a0177868016799b1761c81","amount":8,"dleq":null,"id":"00ffd48b8f5ecf80"},{"C_":"031246ee046519b15648f1b8d8ffcb8e537409c84724e148c8d6800b2e62deb795","amount":2,"dleq":null,"id":"00ffd48b8f5ecf80"}]}"#;

        let res: RestoreResponse = serde_json::from_str(rs).unwrap();

        println!("{:?}", res);
    }
}
