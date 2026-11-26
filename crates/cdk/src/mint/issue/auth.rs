use tracing::instrument;

use crate::mint::nut22::MintAuthRequest;
use crate::mint::{Admission, AuthToken, CurrencyUnit, MintResponse};
use crate::{Amount, Error, Mint};

impl Mint {
    /// Mint Auth Proofs
    #[instrument(skip_all)]
    pub async fn mint_blind_auth(
        &self,
        auth_token: AuthToken,
        mint_auth_request: MintAuthRequest,
    ) -> Result<MintResponse, Error> {
        let cat = if let AuthToken::ClearAuth(cat) = auth_token {
            cat
        } else {
            tracing::debug!("Received blind auth mint without cat");
            return Err(Error::ClearAuthRequired);
        };

        self.verify_clear_auth(cat).await?;

        let auth_settings = self
            .mint_info()
            .await?
            .nuts
            .nut22
            .ok_or(Error::AuthSettingsUndefined)?;

        if mint_auth_request.amount() > auth_settings.bat_max_mint {
            return Err(Error::AmountOutofLimitRange(
                1.into(),
                auth_settings.bat_max_mint.into(),
                mint_auth_request.amount().into(),
            ));
        }

        let outputs = mint_auth_request.outputs;
        if outputs.is_empty() {
            return Ok(MintResponse {
                signatures: Vec::new(),
            });
        }

        if self.verify_outputs_keyset(&outputs)? != CurrencyUnit::Auth {
            return Err(Error::BlindAuthFailed);
        }
        if outputs
            .iter()
            .any(|output| output.amount != Amount::from(1))
        {
            return Err(Error::AmountKey);
        }

        let signatures = self.blind_sign(outputs, Admission::Bounded).await?;
        Ok(MintResponse { signatures })
    }
}
