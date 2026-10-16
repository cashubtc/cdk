//! Payment method administration service.

use std::str::FromStr;

use cdk::nuts::nut04::MintMethodSettings;
use cdk::nuts::nut05::MeltMethodSettings;
use cdk::nuts::{CurrencyUnit, PaymentMethod};
use cdk::Amount;
use tonic::{Request, Response, Status};

use super::MintRPCServer;
use crate::payment_method::payment_method_service_server::PaymentMethodService;

impl MintRPCServer {
    /// Updates the settings of one mint (NUT-04) payment method, keeping the
    /// current value of any setting that is not given
    ///
    /// Returns the method settings in effect after the update.
    async fn set_mint_method(
        &self,
        unit: &str,
        method: &str,
        min_amount: Option<u64>,
        max_amount: Option<u64>,
        options: Option<cdk::nuts::nut04::MintMethodOptions>,
        method_name: Option<String>,
    ) -> Result<MintMethodSettings, Status> {
        self.ensure_mutation_allowed().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let unit = CurrencyUnit::from_str(unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut04_settings = info.nuts.nut04.remove_settings(&unit, &payment_method);

        let updated_method_settings = MintMethodSettings {
            method: payment_method,
            unit,
            method_name: method_name.or_else(|| {
                current_nut04_settings
                    .as_ref()
                    .and_then(|s| s.method_name.clone())
            }),
            min_amount: min_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: max_amount
                .map(Amount::from)
                .or_else(|| current_nut04_settings.as_ref().and_then(|s| s.max_amount)),
            options: options.or_else(|| {
                current_nut04_settings
                    .as_ref()
                    .and_then(|s| s.options.clone())
            }),
        };

        info.nuts
            .nut04
            .methods
            .push(updated_method_settings.clone());

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(updated_method_settings)
    }

    /// Enables or disables minting and melting for the whole mint, keeping
    /// the current value of any flag that is not given
    ///
    /// Returns the (minting disabled, melting disabled) flags in effect after
    /// the update, applied in a single write.
    async fn set_disabled(
        &self,
        mint_disabled: Option<bool>,
        melt_disabled: Option<bool>,
    ) -> Result<(bool, bool), Status> {
        self.ensure_mutation_allowed().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        if mint_disabled.is_none() && melt_disabled.is_none() {
            return Ok((info.nuts.nut04.disabled, info.nuts.nut05.disabled));
        }

        if let Some(disabled) = mint_disabled {
            info.nuts.nut04.disabled = disabled;
        }

        if let Some(disabled) = melt_disabled {
            info.nuts.nut05.disabled = disabled;
        }

        let flags = (info.nuts.nut04.disabled, info.nuts.nut05.disabled);

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(flags)
    }

    /// Updates the settings of one melt (NUT-05) payment method, keeping the
    /// current value of any setting that is not given
    ///
    /// Returns the method settings in effect after the update.
    async fn set_melt_method(
        &self,
        unit: &str,
        method: &str,
        min_amount: Option<u64>,
        max_amount: Option<u64>,
        options: Option<cdk::nuts::nut05::MeltMethodOptions>,
        method_name: Option<String>,
    ) -> Result<MeltMethodSettings, Status> {
        self.ensure_mutation_allowed().await?;
        let mut info = self
            .mint
            .mint_info()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        let unit = CurrencyUnit::from_str(unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let payment_method = PaymentMethod::from_str(method)
            .map_err(|_| Status::invalid_argument("Invalid method".to_string()))?;

        self.mint
            .get_payment_processor(unit.clone(), payment_method.clone())
            .map_err(|_| Status::invalid_argument("Unit payment method pair is not supported"))?;

        let current_nut05_settings = info.nuts.nut05.remove_settings(&unit, &payment_method);

        let updated_method_settings = MeltMethodSettings {
            method: payment_method,
            unit,
            method_name: method_name.or_else(|| {
                current_nut05_settings
                    .as_ref()
                    .and_then(|s| s.method_name.clone())
            }),
            min_amount: min_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.min_amount)),
            max_amount: max_amount
                .map(Amount::from)
                .or_else(|| current_nut05_settings.as_ref().and_then(|s| s.max_amount)),
            options: options.or_else(|| {
                current_nut05_settings
                    .as_ref()
                    .and_then(|s| s.options.clone())
            }),
        };

        info.nuts
            .nut05
            .methods
            .push(updated_method_settings.clone());

        self.mint
            .set_mint_info(info)
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        Ok(updated_method_settings)
    }
}

#[tonic::async_trait]
impl PaymentMethodService for MintRPCServer {
    /// Updates the settings of one mint (NUT-04) payment method
    async fn update_mint_method(
        &self,
        request: Request<crate::payment_method::UpdateMintMethodRequest>,
    ) -> Result<Response<crate::payment_method::UpdateMintMethodResponse>, Status> {
        let request = request.into_inner();

        if request.options.is_some()
            && PaymentMethod::from_str(&request.method).is_ok_and(|method| !method.is_bolt11())
        {
            return Err(Status::invalid_argument(
                "Options can only be set on the bolt11 method".to_string(),
            ));
        }

        let options = request
            .options
            .map(|options| cdk::nuts::nut04::MintMethodOptions::Bolt11 {
                description: options.description,
            });

        let settings = self
            .set_mint_method(
                &request.unit,
                &request.method,
                request.min_amount,
                request.max_amount,
                options,
                request.method_name,
            )
            .await?;

        Ok(Response::new(settings.into()))
    }

    /// Updates the settings of one melt (NUT-05) payment method
    async fn update_melt_method(
        &self,
        request: Request<crate::payment_method::UpdateMeltMethodRequest>,
    ) -> Result<Response<crate::payment_method::UpdateMeltMethodResponse>, Status> {
        let request = request.into_inner();

        if request.options.is_some()
            && PaymentMethod::from_str(&request.method).is_ok_and(|method| !method.is_bolt11())
        {
            return Err(Status::invalid_argument(
                "Options can only be set on the bolt11 method".to_string(),
            ));
        }

        let options = request
            .options
            .map(|options| cdk::nuts::nut05::MeltMethodOptions::Bolt11 {
                amountless: options.amountless,
            });

        let settings = self
            .set_melt_method(
                &request.unit,
                &request.method,
                request.min_amount,
                request.max_amount,
                options,
                request.method_name,
            )
            .await?;

        Ok(Response::new(settings.into()))
    }

    /// Enables or disables minting and melting for the whole mint
    async fn update_disabled(
        &self,
        request: Request<crate::payment_method::UpdateDisabledRequest>,
    ) -> Result<Response<crate::payment_method::UpdateDisabledResponse>, Status> {
        let request = request.into_inner();

        let (mint_disabled, melt_disabled) = self
            .set_disabled(request.mint_disabled, request.melt_disabled)
            .await?;

        Ok(Response::new(
            crate::payment_method::UpdateDisabledResponse {
                mint_disabled,
                melt_disabled,
            },
        ))
    }
}

impl From<MintMethodSettings> for crate::payment_method::UpdateMintMethodResponse {
    fn from(settings: MintMethodSettings) -> Self {
        let options = settings.options.and_then(|options| match options {
            cdk::nuts::nut04::MintMethodOptions::Bolt11 { description }
            | cdk::nuts::nut04::MintMethodOptions::Bolt12 { description } => {
                Some(crate::payment_method::Bolt11MintMethodOptions { description })
            }
            _ => None,
        });

        Self {
            unit: settings.unit.to_string(),
            method: settings.method.to_string(),
            min_amount: settings.min_amount.map(u64::from),
            max_amount: settings.max_amount.map(u64::from),
            options,
            method_name: settings.method_name,
        }
    }
}

impl From<MeltMethodSettings> for crate::payment_method::UpdateMeltMethodResponse {
    fn from(settings: MeltMethodSettings) -> Self {
        let options = settings.options.map(|options| match options {
            cdk::nuts::nut05::MeltMethodOptions::Bolt11 { amountless } => {
                crate::payment_method::Bolt11MeltMethodOptions { amountless }
            }
        });

        Self {
            unit: settings.unit.to_string(),
            method: settings.method.to_string(),
            min_amount: settings.min_amount.map(u64::from),
            max_amount: settings.max_amount.map(u64::from),
            options,
            method_name: settings.method_name,
        }
    }
}

#[cfg(test)]
mod tests {
    use cdk_common::nut00::KnownMethod;
    use tonic::Code;

    use super::super::test_utils::create_test_rpc_server;
    use super::*;

    #[test]
    fn test_update_mint_method_response_maps_bolt12_description_options() {
        let settings = MintMethodSettings {
            method: PaymentMethod::Known(KnownMethod::Bolt12),
            unit: CurrencyUnit::Sat,
            method_name: Some("Bolt12".to_string()),
            min_amount: Some(Amount::from(1)),
            max_amount: Some(Amount::from(1_000)),
            options: Some(cdk::nuts::nut04::MintMethodOptions::Bolt12 { description: true }),
        };

        let response: crate::payment_method::UpdateMintMethodResponse = settings.into();

        assert_eq!(response.unit, "sat");
        assert_eq!(response.method, "bolt12");
        assert_eq!(response.min_amount, Some(1));
        assert_eq!(response.max_amount, Some(1_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MintMethodOptions { description: true })
        );
        assert_eq!(response.method_name.as_deref(), Some("Bolt12"));
    }

    #[tokio::test]
    async fn test_payment_method_service_update_mint_method_keeps_omitted_settings() {
        let server = create_test_rpc_server().await;

        let response = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: Some(1),
                max_amount: Some(1_000),
                options: Some(crate::payment_method::Bolt11MintMethodOptions { description: true }),
                method_name: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.unit, "sat");
        assert_eq!(response.method, "bolt11");
        assert_eq!(response.min_amount, Some(1));
        assert_eq!(response.max_amount, Some(1_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MintMethodOptions { description: true })
        );

        let response = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: None,
                max_amount: Some(5_000),
                options: None,
                method_name: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.min_amount, Some(1));
        assert_eq!(response.max_amount, Some(5_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MintMethodOptions { description: true })
        );

        let settings = server
            .mint
            .mint_info()
            .await
            .unwrap()
            .nuts
            .nut04
            .get_settings(
                &CurrencyUnit::Sat,
                &PaymentMethod::Known(KnownMethod::Bolt11),
            )
            .unwrap();
        assert_eq!(settings.min_amount, Some(Amount::from(1)));
        assert_eq!(settings.max_amount, Some(Amount::from(5_000)));
    }

    #[tokio::test]
    async fn test_payment_method_service_update_mint_method_rejects_unknown_pair() {
        let server = create_test_rpc_server().await;

        let error = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt12".to_owned(),
                min_amount: None,
                max_amount: None,
                options: None,
                method_name: None,
            }),
        )
        .await
        .expect_err("method without a payment processor should be rejected");

        // The message separates this from the options guard, which must not
        // fire on a request that carries no options
        assert_eq!(error.code(), Code::InvalidArgument);
        assert_eq!(error.message(), "Unit payment method pair is not supported");
    }

    #[tokio::test]
    async fn test_payment_method_service_rejects_options_on_non_bolt11_method() {
        let server = create_test_rpc_server().await;

        let error = PaymentMethodService::update_mint_method(
            &server,
            Request::new(crate::payment_method::UpdateMintMethodRequest {
                unit: "sat".to_owned(),
                method: "onchain".to_owned(),
                min_amount: None,
                max_amount: None,
                options: Some(crate::payment_method::Bolt11MintMethodOptions { description: true }),
                method_name: None,
            }),
        )
        .await
        .expect_err("bolt11 options on an onchain method should be rejected");

        // The processor check also rejects this pair; the message shows the guard fired
        assert_eq!(error.code(), Code::InvalidArgument);
        assert_eq!(
            error.message(),
            "Options can only be set on the bolt11 method"
        );

        let error = PaymentMethodService::update_melt_method(
            &server,
            Request::new(crate::payment_method::UpdateMeltMethodRequest {
                unit: "sat".to_owned(),
                method: "onchain".to_owned(),
                min_amount: None,
                max_amount: None,
                options: Some(crate::payment_method::Bolt11MeltMethodOptions { amountless: true }),
                method_name: None,
            }),
        )
        .await
        .expect_err("bolt11 options on an onchain method should be rejected");

        assert_eq!(error.code(), Code::InvalidArgument);
        assert_eq!(
            error.message(),
            "Options can only be set on the bolt11 method"
        );
    }

    #[tokio::test]
    async fn test_payment_method_service_update_disabled_keeps_omitted_flag() {
        let server = create_test_rpc_server().await;

        let response = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: Some(true),
                melt_disabled: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert!(response.mint_disabled);
        assert!(!response.melt_disabled);
        let info = server.mint.mint_info().await.unwrap();
        assert!(info.nuts.nut04.disabled);
        assert!(!info.nuts.nut05.disabled);

        let response = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: Some(false),
                melt_disabled: Some(true),
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert!(!response.mint_disabled);
        assert!(response.melt_disabled);
        let info = server.mint.mint_info().await.unwrap();
        assert!(!info.nuts.nut04.disabled);
        assert!(info.nuts.nut05.disabled);
    }

    #[tokio::test]
    async fn test_payment_method_service_update_disabled_with_no_flags_changes_nothing() {
        let server = create_test_rpc_server().await;

        // Set both flags first; on a fresh server every flag is already false
        PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: Some(true),
                melt_disabled: Some(true),
            }),
        )
        .await
        .unwrap();

        let response = PaymentMethodService::update_disabled(
            &server,
            Request::new(crate::payment_method::UpdateDisabledRequest {
                mint_disabled: None,
                melt_disabled: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert!(response.mint_disabled);
        assert!(response.melt_disabled);
        let info = server.mint.mint_info().await.unwrap();
        assert!(info.nuts.nut04.disabled);
        assert!(info.nuts.nut05.disabled);
    }

    #[tokio::test]
    async fn test_payment_method_service_update_melt_method_keeps_omitted_settings() {
        let server = create_test_rpc_server().await;

        PaymentMethodService::update_melt_method(
            &server,
            Request::new(crate::payment_method::UpdateMeltMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: Some(2),
                max_amount: Some(2_000),
                options: Some(crate::payment_method::Bolt11MeltMethodOptions { amountless: true }),
                method_name: None,
            }),
        )
        .await
        .unwrap();

        let response = PaymentMethodService::update_melt_method(
            &server,
            Request::new(crate::payment_method::UpdateMeltMethodRequest {
                unit: "sat".to_owned(),
                method: "bolt11".to_owned(),
                min_amount: None,
                max_amount: None,
                options: None,
                method_name: Some("Lightning".to_owned()),
            }),
        )
        .await
        .unwrap()
        .into_inner();

        assert_eq!(response.min_amount, Some(2));
        assert_eq!(response.max_amount, Some(2_000));
        assert_eq!(
            response.options,
            Some(crate::payment_method::Bolt11MeltMethodOptions { amountless: true })
        );
        assert_eq!(response.method_name, Some("Lightning".to_owned()));
    }
}
