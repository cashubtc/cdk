use tracing::instrument;

use crate::mint::nut22::MintAuthRequest;
use crate::mint::{AuthToken, MintBolt11Response};
use crate::{Amount, Error, Mint};

impl Mint {
    /// Mint Auth Proofs
    #[instrument(skip_all)]
    pub async fn mint_blind_auth(
        &self,
        auth_token: AuthToken,
        mint_auth_request: MintAuthRequest,
    ) -> Result<MintBolt11Response, Error> {
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

        let mut blind_signatures = Vec::with_capacity(mint_auth_request.outputs.len());

        for blinded_message in mint_auth_request.outputs.iter() {
            if blinded_message.amount != Amount::from(1) {
                return Err(Error::AmountKey);
            }

            let blind_signature = self.auth_blind_sign(blinded_message).await?;
            blind_signatures.push(blind_signature);
        }

        Ok(MintBolt11Response {
            signatures: blind_signatures,
        })
    }
}
