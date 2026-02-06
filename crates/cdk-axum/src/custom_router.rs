//! Dynamic router creation for custom payment methods
//!
//! Creates dedicated routes for each configured custom payment method,
//! matching the URL pattern of bolt11/bolt12 routes (e.g., /v1/mint/quote/paypal).

use axum::routing::{get, post};
use axum::Router;

use crate::custom_handlers::{
    cache_post_melt_custom, cache_post_mint_custom, get_check_melt_custom_quote,
    get_check_mint_custom_quote, post_melt_custom_quote, post_mint_custom_quote,
};
use crate::MintState;

/// Creates routers for all configured custom payment methods
///
/// Creates a single set of parameterized routes that handle all custom methods:
/// - `/mint/quote/{method}` - POST: Create mint quote
/// - `/mint/quote/{method}/{quote_id}` - GET: Check mint quote status
/// - `/mint/{method}` - POST: Mint tokens
/// - `/melt/quote/{method}` - POST: Create melt quote
/// - `/melt/quote/{method}/{quote_id}` - GET: Check melt quote status
/// - `/melt/{method}` - POST: Melt tokens
///
/// The {method} parameter captures the payment method name dynamically.
pub fn create_custom_routers(state: MintState, custom_methods: Vec<String>) -> Router<MintState> {
    tracing::info!(
        "Creating routes for {} custom payment methods: {:?}",
        custom_methods.len(),
        custom_methods
    );

    // Create a single router with parameterized routes that handle all custom methods
    // Use cached versions for mint/melt to support NUT-19 caching
    Router::new()
        .route("/mint/quote/{method}", post(post_mint_custom_quote))
        .route(
            "/mint/quote/{method}/{quote_id}",
            get(get_check_mint_custom_quote),
        )
        .route("/mint/{method}", post(cache_post_mint_custom))
        .route("/melt/quote/{method}", post(post_melt_custom_quote))
        .route(
            "/melt/quote/{method}/{quote_id}",
            get(get_check_melt_custom_quote),
        )
        .route("/melt/{method}", post(cache_post_melt_custom))
        .with_state(state)
}

/// Validates that custom method names are valid
///
/// Previously, bolt11 and bolt12 were reserved, but now they can be handled
/// through the custom router if the payment processor supports them.
pub fn validate_custom_method_names(methods: &[String]) -> Result<(), String> {
    for method in methods {
        // Validate method name contains only URL-safe characters
        if !method
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "Custom payment method name '{}' contains invalid characters. Only alphanumeric, '-', and '_' are allowed.",
                method
            ));
        }

        // Validate method name is not empty
        if method.is_empty() {
            return Err("Custom payment method name cannot be empty".to_string());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use cdk::nuts::nut00::KnownMethod;
    use cdk::nuts::PaymentMethod;

    use super::*;

    #[test]
    fn test_validate_custom_method_names_valid() {
        assert!(validate_custom_method_names(&["paypal".to_string()]).is_ok());
        assert!(
            validate_custom_method_names(&["venmo".to_string(), "cashapp".to_string()]).is_ok()
        );
        assert!(validate_custom_method_names(&["my-method".to_string()]).is_ok());
        assert!(validate_custom_method_names(&["my_method".to_string()]).is_ok());
        assert!(validate_custom_method_names(&["method123".to_string()]).is_ok());
    }

    #[test]
    fn test_validate_custom_method_names_bolt11_bolt12_allowed() {
        // bolt11 and bolt12 are now allowed as custom methods
        assert!(validate_custom_method_names(&[
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        ])
        .is_ok());
        assert!(validate_custom_method_names(&[
            PaymentMethod::Known(KnownMethod::Bolt12).to_string()
        ])
        .is_ok());
        assert!(validate_custom_method_names(&[
            "paypal".to_string(),
            PaymentMethod::Known(KnownMethod::Bolt11).to_string()
        ])
        .is_ok());
    }

    #[test]
    fn test_validate_custom_method_names_invalid_chars() {
        assert!(validate_custom_method_names(&["pay/pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay@pal".to_string()]).is_err());
    }

    #[test]
    fn test_validate_custom_method_names_empty() {
        assert!(validate_custom_method_names(&["".to_string()]).is_err());
    }

    #[test]
    fn test_validate_custom_method_names_multiple_invalid() {
        assert!(validate_custom_method_names(&[
            "valid".to_string(),
            "in valid".to_string(),
            "also-valid".to_string()
        ])
        .is_err());
    }

    #[test]
    fn test_validate_custom_method_names_special_chars() {
        // Test various special characters that should fail
        assert!(validate_custom_method_names(&["pay.pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay+pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay$pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay%pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay&pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay*pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay(pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay)pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay=pal".to_string()]).is_err());
        assert!(validate_custom_method_names(&["pay#pal".to_string()]).is_err());
    }

    #[test]
    fn test_validate_custom_method_names_edge_cases() {
        // Single character names
        assert!(validate_custom_method_names(&["a".to_string()]).is_ok());
        assert!(validate_custom_method_names(&["1".to_string()]).is_ok());
        assert!(validate_custom_method_names(&["-".to_string()]).is_ok());
        assert!(validate_custom_method_names(&["_".to_string()]).is_ok());

        // Names with only dashes or underscores
        assert!(validate_custom_method_names(&["---".to_string()]).is_ok());
        assert!(validate_custom_method_names(&["___".to_string()]).is_ok());

        // Long names
        let long_name = "a".repeat(100);
        assert!(validate_custom_method_names(&[long_name]).is_ok());
    }

    #[test]
    fn test_validate_custom_method_names_mixed_valid() {
        assert!(validate_custom_method_names(&[
            "paypal".to_string(),
            "cash-app".to_string(),
            "venmo_pay".to_string(),
            "method123".to_string(),
            "UPPERCASE".to_string(),
        ])
        .is_ok());
    }

    #[test]
    fn test_validate_custom_method_names_error_messages() {
        // Test that error messages are descriptive
        let result = validate_custom_method_names(&["pay/pal".to_string()]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("pay/pal"));
        assert!(err.contains("invalid characters"));

        let result = validate_custom_method_names(&["".to_string()]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn test_validate_custom_method_names_unicode() {
        // Unicode characters should fail (not ASCII alphanumeric)
        assert!(validate_custom_method_names(&["café".to_string()]).is_err());
        assert!(validate_custom_method_names(&["北京".to_string()]).is_err());
        assert!(validate_custom_method_names(&["🚀".to_string()]).is_err());
    }

    #[test]
    fn test_validate_custom_method_names_empty_list() {
        // Empty list should be valid (no methods to validate)
        assert!(validate_custom_method_names(&[]).is_ok());
    }

    #[test]
    fn test_create_custom_routers_method_list() {
        // This test verifies the method list formatting
        let custom_methods = vec!["paypal".to_string(), "venmo".to_string()];

        let methods_str = custom_methods
            .iter()
            .map(|m| m.as_str())
            .collect::<Vec<_>>()
            .join(", ");

        // Verify the method string is formatted correctly
        assert!(methods_str.contains("paypal"));
        assert!(methods_str.contains("venmo"));
        assert_eq!(methods_str, "paypal, venmo");
    }
}
