use tracing::instrument;

use crate::mint::nutxx1::MintAuthRequest;
use crate::mint::{AuthToken, MintBolt11Response, PublicKey};
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
            return Err(Error::AuthRequired);
        };

        self.verify_clear_auth(cat)?;

        if mint_auth_request.amount() > self.mint_info().nuts.nutxx1.bat_max_mint {
            return Err(Error::AmountOutofLimitRange(
                1.into(),
                self.mint_info().nuts.nutxx1.bat_max_mint.into(),
                mint_auth_request.amount().into(),
            ));
        }

        let mut blind_signatures = Vec::with_capacity(mint_auth_request.outputs.len());

        for blinded_message in mint_auth_request.outputs.iter() {
            if blinded_message.amount != Amount::from(1) {
                return Err(Error::AmountKey);
            }

            let blind_signature = self.blind_sign(blinded_message).await?;
            blind_signatures.push(blind_signature);
        }

        self.localstore
            .add_blind_signatures(
                &mint_auth_request
                    .outputs
                    .iter()
                    .map(|p| p.blinded_secret)
                    .collect::<Vec<PublicKey>>(),
                &blind_signatures,
                None,
            )
            .await?;

        Ok(MintBolt11Response {
            signatures: blind_signatures,
        })
    }
}
