//! Builder patterns for MultiMintWallet operations
//!
//! These builders provide a fluent interface for constructing complex
//! operations with the MultiMintWallet.

use std::sync::Arc;

use crate::amount::Amount;
use crate::mint_url::MintUrl;
use crate::nuts::{MeltOptions, SpendingConditions, Token};
use crate::types::Melted;
use crate::wallet::{MultiMintWallet, SendOptions};

use super::Error;

/// Builder for complex send operations
pub struct SendBuilder {
    wallet: Arc<MultiMintWallet>,
    amount: Amount,
    options: SendOptions,
    preferred_mint: Option<MintUrl>,
    fallback_to_any: bool,
    max_fee: Option<Amount>,
}

impl SendBuilder {
    /// Create a new SendBuilder
    pub fn new(wallet: Arc<MultiMintWallet>, amount: Amount) -> Self {
        Self {
            wallet,
            amount,
            options: SendOptions::default(),
            preferred_mint: None,
            fallback_to_any: true,
            max_fee: None,
        }
    }

    /// Set send options
    pub fn with_options(mut self, options: SendOptions) -> Self {
        self.options = options;
        self
    }

    /// Set spending conditions (P2PK, HTLC, etc.)
    pub fn with_conditions(mut self, conditions: SpendingConditions) -> Self {
        self.options.conditions = Some(conditions);
        self
    }

    /// Include fee in the token
    pub fn include_fee(mut self, include: bool) -> Self {
        self.options.include_fee = include;
        self
    }

    /// Prefer a specific mint
    pub fn prefer_mint(mut self, mint_url: MintUrl) -> Self {
        self.preferred_mint = Some(mint_url);
        self
    }

    /// Whether to fallback to any available wallet if preferred mint fails
    pub fn fallback_to_any(mut self, fallback: bool) -> Self {
        self.fallback_to_any = fallback;
        self
    }

    /// Set maximum acceptable fee
    pub fn max_fee(mut self, max_fee: Amount) -> Self {
        self.max_fee = Some(max_fee);
        self
    }

    /// Execute the send operation
    pub async fn send(self) -> Result<Token, Error> {
        // Try preferred mint first if specified
        if let Some(mint_url) = self.preferred_mint {
            match self.wallet.send_from_wallet(&mint_url, self.amount, self.options.clone()).await {
                Ok(token) => return Ok(token),
                Err(e) if !self.fallback_to_any => return Err(e),
                Err(_) => {
                    // Continue to automatic selection
                    tracing::debug!("Preferred mint failed, falling back to automatic selection");
                }
            }
        }

        // Use automatic wallet selection
        self.wallet.send(self.amount, self.options).await
    }
}

/// Builder for complex melt (payment) operations
pub struct MeltBuilder {
    wallet: Arc<MultiMintWallet>,
    bolt11: String,
    options: Option<MeltOptions>,
    preferred_mint: Option<MintUrl>,
    enable_mpp: bool,
    max_fee: Option<Amount>,
    max_mpp_parts: usize,
}

impl MeltBuilder {
    /// Create a new MeltBuilder
    pub fn new(wallet: Arc<MultiMintWallet>, bolt11: String) -> Self {
        Self {
            wallet,
            bolt11,
            options: None,
            preferred_mint: None,
            enable_mpp: true,
            max_fee: None,
            max_mpp_parts: 3,
        }
    }

    /// Set melt options
    pub fn with_options(mut self, options: MeltOptions) -> Self {
        self.options = Some(options);
        self
    }

    /// Prefer a specific mint for payment
    pub fn prefer_mint(mut self, mint_url: MintUrl) -> Self {
        self.preferred_mint = Some(mint_url);
        self
    }

    /// Enable or disable Multi-Path Payment
    pub fn enable_mpp(mut self, enable: bool) -> Self {
        self.enable_mpp = enable;
        self
    }

    /// Set maximum acceptable fee
    pub fn max_fee(mut self, max_fee: Amount) -> Self {
        self.max_fee = Some(max_fee);
        self
    }

    /// Set maximum number of MPP parts (if MPP is enabled)
    pub fn max_mpp_parts(mut self, max_parts: usize) -> Self {
        self.max_mpp_parts = max_parts;
        self
    }

    /// Execute the melt operation
    pub async fn pay(self) -> Result<Melted, Error> {
        // Try preferred mint first if specified
        if let Some(mint_url) = self.preferred_mint {
            match self.wallet.melt_from_wallet(
                &mint_url, 
                &self.bolt11, 
                self.options.clone(), 
                self.max_fee
            ).await {
                Ok(melted) => return Ok(melted),
                Err(e) if !self.enable_mpp => return Err(e),
                Err(_) => {
                    // Continue to automatic selection/MPP
                    tracing::debug!("Preferred mint failed, falling back to automatic selection");
                }
            }
        }

        // Use automatic wallet selection (with MPP if enabled)
        self.wallet.melt(&self.bolt11, self.options, self.max_fee).await
    }
}

/// Builder for complex swap operations
pub struct SwapBuilder {
    wallet: Arc<MultiMintWallet>,
    amount: Option<Amount>,
    conditions: Option<SpendingConditions>,
    preferred_mint: Option<MintUrl>,
    consolidate: bool,
}

impl SwapBuilder {
    /// Create a new SwapBuilder
    pub fn new(wallet: Arc<MultiMintWallet>) -> Self {
        Self {
            wallet,
            amount: None,
            conditions: None,
            preferred_mint: None,
            consolidate: false,
        }
    }

    /// Set the amount to swap
    pub fn amount(mut self, amount: Amount) -> Self {
        self.amount = Some(amount);
        self
    }

    /// Set spending conditions for the new proofs
    pub fn with_conditions(mut self, conditions: SpendingConditions) -> Self {
        self.conditions = Some(conditions);
        self
    }

    /// Prefer a specific mint for the swap
    pub fn prefer_mint(mut self, mint_url: MintUrl) -> Self {
        self.preferred_mint = Some(mint_url);
        self
    }

    /// Whether to consolidate proofs during swap
    pub fn consolidate(mut self, consolidate: bool) -> Self {
        self.consolidate = consolidate;
        self
    }

    /// Execute the swap operation
    pub async fn swap(self) -> Result<Option<crate::nuts::Proofs>, Error> {
        if self.consolidate {
            // If consolidation is requested, do that instead
            self.wallet.consolidate().await?;
            return Ok(None);
        }

        // Use automatic wallet selection for swap
        self.wallet.swap(self.amount, self.conditions).await
    }
}

/// Extension trait to add builder methods to MultiMintWallet
pub trait MultiMintWalletBuilderExt {
    /// Create a send builder
    fn send_builder(&self, amount: Amount) -> SendBuilder;
    
    /// Create a melt builder
    fn melt_builder(&self, bolt11: String) -> MeltBuilder;
    
    /// Create a swap builder
    fn swap_builder(&self) -> SwapBuilder;
}

impl MultiMintWalletBuilderExt for Arc<MultiMintWallet> {
    fn send_builder(&self, amount: Amount) -> SendBuilder {
        SendBuilder::new(self.clone(), amount)
    }

    fn melt_builder(&self, bolt11: String) -> MeltBuilder {
        MeltBuilder::new(self.clone(), bolt11)
    }

    fn swap_builder(&self) -> SwapBuilder {
        SwapBuilder::new(self.clone())
    }
}