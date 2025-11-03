//! Dynamic router creation for custom payment methods
//!
//! Creates dedicated routes for each configured custom payment method,
//! matching the URL pattern of bolt11/bolt12 routes (e.g., /v1/mint/quote/paypal).

use axum::routing::{get, post};
use axum::Router;

use crate::custom_handlers::{
    get_check_melt_custom_quote, get_check_mint_custom_quote, post_melt_custom,
    post_melt_custom_quote, post_mint_custom, post_mint_custom_quote,
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
    Router::new()
        .route("/mint/quote/{method}", post(post_mint_custom_quote))
        .route(
            "/mint/quote/{method}/{quote_id}",
            get(get_check_mint_custom_quote),
        )
        .route("/mint/{method}", post(post_mint_custom))
        .route("/melt/quote/{method}", post(post_melt_custom_quote))
        .route(
            "/melt/quote/{method}/{quote_id}",
            get(get_check_melt_custom_quote),
        )
        .route("/melt/{method}", post(post_melt_custom))
        .with_state(state)
}

/// Validates that custom method names don't conflict with reserved names
///
/// Reserved names are payment methods already handled by dedicated code:
/// - "bolt11" - Lightning BOLT11 invoices
/// - "bolt12" - Lightning BOLT12 offers
pub fn validate_custom_method_names(methods: &[String]) -> Result<(), String> {
    const RESERVED_METHODS: &[&str] = &["bolt11", "bolt12"];

    for method in methods {
        if RESERVED_METHODS.contains(&method.as_str()) {
            return Err(format!(
                "Custom payment method name '{}' is reserved. Please use a different name.",
                method
            ));
        }

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
    fn test_validate_custom_method_names_reserved() {
        assert!(validate_custom_method_names(&["bolt11".to_string()]).is_err());
        assert!(validate_custom_method_names(&["bolt12".to_string()]).is_err());
        assert!(
            validate_custom_method_names(&["paypal".to_string(), "bolt11".to_string()]).is_err()
        );
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
}
