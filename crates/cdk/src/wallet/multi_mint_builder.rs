//! Builder patterns for MultiMintWallet operations
//!
//! These builders provide a fluent interface for constructing complex
//! operations with the MultiMintWallet.

use std::sync::Arc;

use super::multi_mint_wallet::MultiMintSendOptions;
use super::Error;
use crate::amount::Amount;
use crate::mint_url::MintUrl;
use crate::nuts::{MeltOptions, SpendingConditions, Token};
use crate::types::Melted;
use crate::wallet::{MultiMintWallet, SendOptions};

/// Builder for complex send operations with advanced features
///
/// # Examples
///
/// ```no_run
/// # use cdk::wallet::{MultiMintWallet, SendBuilder};
/// # use cdk::{Amount, mint_url::MintUrl};
/// # use std::sync::Arc;
/// # async fn example(wallet: Arc<MultiMintWallet>) -> Result<(), Box<dyn std::error::Error>> {
/// let mint_url: MintUrl = "https://mint.example.com".parse()?;
/// 
/// // Simple send from a specific mint
/// let token = SendBuilder::new(wallet.clone(), Amount::from(100), mint_url.clone())
///     .send()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct SendBuilder {
    wallet: Arc<MultiMintWallet>,
    amount: Amount,
    mint_url: MintUrl,
    options: MultiMintSendOptions,
    max_fee: Option<Amount>,
}

impl SendBuilder {
    /// Create a new SendBuilder with a required mint URL
    /// Since the new interface requires specifying a mint, you must choose which mint to send from
    pub fn new(wallet: Arc<MultiMintWallet>, amount: Amount, mint_url: MintUrl) -> Self {
        Self {
            wallet,
            amount,
            mint_url,
            options: MultiMintSendOptions::new(),
            max_fee: None,
        }
    }

    /// Set send options
    ///
    /// This configures advanced sending behavior like proof selection strategy,
    /// spending conditions, and more.
    pub fn with_send_options(mut self, options: SendOptions) -> Self {
        self.options = self.options.send_options(options);
        self
    }

    /// Set multi-mint send options
    ///
    /// This allows fine-grained control over mint selection and cross-mint behavior.
    pub fn with_multi_mint_options(mut self, options: MultiMintSendOptions) -> Self {
        self.options = options;
        self
    }

    /// Set spending conditions (P2PK, HTLC, etc.)
    pub fn with_conditions(mut self, conditions: SpendingConditions) -> Self {
        let mut send_opts = self.options.send_options.clone();
        send_opts.conditions = Some(conditions);
        self.options = self.options.send_options(send_opts);
        self
    }

    /// Include fee in the token
    pub fn include_fee(mut self, include: bool) -> Self {
        let mut send_opts = self.options.send_options.clone();
        send_opts.include_fee = include;
        self.options = self.options.send_options(send_opts);
        self
    }

    /// Enable transferring funds from other mints if the sending mint doesn't have sufficient balance
    pub fn allow_transfer(mut self, allow: bool) -> Self {
        self.options = self.options.allow_transfer(allow);
        self
    }

    /// Set maximum amount to transfer from other mints
    pub fn max_transfer_amount(mut self, amount: Amount) -> Self {
        self.options = self.options.max_transfer_amount(amount);
        self
    }

    /// Set maximum acceptable fee
    pub fn max_fee(mut self, max_fee: Amount) -> Self {
        self.max_fee = Some(max_fee);
        self
    }

    /// Execute the send operation
    ///
    /// This will prepare and execute the send from the specified mint.
    ///
    /// # Examples
    /// ```no_run
    /// # use cdk::wallet::{MultiMintWallet, SendBuilder};
    /// # use cdk::{Amount, mint_url::MintUrl};
    /// # use std::sync::Arc;
    /// # async fn example(wallet: Arc<MultiMintWallet>) -> Result<(), Box<dyn std::error::Error>> {
    /// let mint_url: MintUrl = "https://mint.example.com".parse()?;
    /// let token = SendBuilder::new(wallet.clone(), Amount::from(100), mint_url)
    ///     .send()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn send(self) -> Result<Token, Error> {
        // Prepare and confirm the send from the specified mint
        let prepared = self
            .wallet
            .prepare_send(self.mint_url, self.amount, self.options)
            .await?;
        let token = prepared.confirm(None).await?;
        Ok(token)
    }
}

/// Builder for complex melt (payment) operations
///
/// Provides a fluent interface for configuring lightning payments with
/// advanced features like multi-path payments, fee limits, and mint preferences.
///
/// # Examples
/// ```no_run
/// # use cdk::wallet::MeltBuilder;
/// # use cdk::Amount;
/// # use cdk::wallet::MultiMintWallet;
/// # use std::sync::Arc;
/// # async fn example(wallet: Arc<MultiMintWallet>) -> Result<(), Box<dyn std::error::Error>> {
/// // Simple payment
/// let result = MeltBuilder::new(wallet.clone(), "lnbc100n1p...".to_string())
///     .pay()
///     .await?;
///
/// // Payment with fee limit and preferred mint
/// let result = MeltBuilder::new(wallet.clone(), "lnbc100n1p...".to_string())
///     .max_fee(Amount::from(10))
///     .prefer_mint("https://preferred.mint".parse()?)
///     .enable_mpp(true)  // Enable multi-path payments
///     .pay()
///     .await?;
/// # Ok(())
/// # }
/// ```
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
    ///
    /// When enabled, the payment can be split across multiple mints if a single
    /// mint doesn't have sufficient balance. This increases payment success rate
    /// but may result in higher total fees.
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
    ///
    /// Attempts to pay the lightning invoice using the configured options.
    /// Will automatically select the best mint(s) based on balance, fees,
    /// and route availability.
    ///
    /// # Returns
    /// - `Ok(Melted)` with payment details on success
    /// - `Err(Error)` if payment fails
    ///
    /// # Example
    /// ```no_run
    /// # use cdk::wallet::{MeltBuilder, MultiMintWallet};
    /// # use std::sync::Arc;
    /// # async fn example(wallet: Arc<MultiMintWallet>) -> Result<(), Box<dyn std::error::Error>> {
    /// let result = MeltBuilder::new(wallet, "lnbc...".to_string())
    ///     .pay()
    ///     .await?;
    /// println!("Payment successful! Paid {} sats with {} sats fee",
    ///          result.amount, result.fee_paid);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn pay(self) -> Result<Melted, Error> {
        // Try preferred mint first if specified
        if let Some(mint_url) = self.preferred_mint {
            match self
                .wallet
                .pay_invoice_for_wallet(&mint_url, &self.bolt11, self.options, self.max_fee)
                .await
            {
                Ok(melted) => return Ok(melted),
                Err(e) if !self.enable_mpp => return Err(e),
                Err(_) => {
                    // Continue to automatic selection/MPP
                    tracing::debug!("Preferred mint failed, falling back to automatic selection");
                }
            }
        }

        // Use automatic wallet selection (with MPP if enabled)
        self.wallet
            .melt(&self.bolt11, self.options, self.max_fee)
            .await
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
    /// Create a send builder for a specific mint
    fn send_builder(&self, amount: Amount, mint_url: MintUrl) -> SendBuilder;

    /// Create a melt builder
    fn melt_builder(&self, bolt11: String) -> MeltBuilder;

    /// Create a swap builder
    fn swap_builder(&self) -> SwapBuilder;
}

impl MultiMintWalletBuilderExt for Arc<MultiMintWallet> {
    fn send_builder(&self, amount: Amount, mint_url: MintUrl) -> SendBuilder {
        SendBuilder::new(self.clone(), amount, mint_url)
    }

    fn melt_builder(&self, bolt11: String) -> MeltBuilder {
        MeltBuilder::new(self.clone(), bolt11)
    }

    fn swap_builder(&self) -> SwapBuilder {
        SwapBuilder::new(self.clone())
    }
}
