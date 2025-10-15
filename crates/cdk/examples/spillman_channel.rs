//! Example: Spillman (Unidirectional) Payment Channel
//!
//! This example will demonstrate a Cashu implementation of Spillman channels,
//! allowing Alice and Bob to set up an offline unidirectional payment channel.
//!
//! Current implementation:
//! - Creating a local mint with FakeWallet backend
//! - Alice creating a token locked to 2-of-2 multisig (Alice + Bob)
//! - Both parties collaboratively redeeming the token
//! - Showing that a single signature fails to redeem
//!
//! TODO: Evolve into full Spillman channel with:
//! - Powers-of-2 denomination proofs
//! - Special 1-millisat proof for double-spend prevention
//! - Incremental signature updates for balance changes
//! - Bob's unilateral exit capability

use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter};
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use bip39::Mnemonic;
use cashu::quote_id::QuoteId;
use cashu::{MeltQuoteBolt12Request, MintQuoteBolt12Request, MintQuoteBolt12Response};
use cdk::mint::{MintBuilder, MintMeltLimits};
use cdk::nuts::nut11::{Conditions, SigFlag};
use cdk::nuts::{
    CheckStateRequest, CheckStateResponse, CurrencyUnit, Id, KeySet, KeysetResponse,
    MeltQuoteBolt11Request, MeltQuoteBolt11Response, MeltRequest, MintInfo,
    MintQuoteBolt11Request, MintQuoteBolt11Response, MintRequest, MintResponse, PaymentMethod,
    RestoreRequest, RestoreResponse, SecretKey, SpendingConditions, SwapRequest, SwapResponse,
};
use cdk::types::{FeeReserve, QuoteTTL};
use cdk::util::unix_time;
use cdk::wallet::{AuthWallet, MintConnector, ReceiveOptions, SendOptions, WalletBuilder};
use cdk::{Error, Mint, StreamExt};
use cdk_fake_wallet::FakeWallet;
use tokio::sync::RwLock;

/// Parameters for a Spillman payment channel
#[derive(Debug, Clone)]
struct SpillmanChannelParameters {
    /// Alice's public key (sender)
    alice_pubkey: cdk::nuts::PublicKey,
    /// Bob's public key (receiver)
    bob_pubkey: cdk::nuts::PublicKey,
    /// Currency unit for the channel
    unit: CurrencyUnit,
    /// Total channel capacity (must be a power of 2)
    capacity: u64,
    /// Locktime after which Alice can reclaim funds (unix timestamp)
    locktime: u64,
}

impl SpillmanChannelParameters {
    /// Create new channel parameters
    ///
    /// # Errors
    ///
    /// Returns an error if capacity is not a power of 2
    fn new(
        alice_pubkey: cdk::nuts::PublicKey,
        bob_pubkey: cdk::nuts::PublicKey,
        unit: CurrencyUnit,
        capacity: u64,
        locktime: u64,
    ) -> anyhow::Result<Self> {
        // Check that capacity is a power of 2
        if capacity == 0 {
            anyhow::bail!("Capacity must be greater than 0");
        }

        let mut i = 1u64;
        while i < capacity {
            i = i.checked_mul(2).ok_or_else(|| {
                anyhow::anyhow!("Capacity {} is too large", capacity)
            })?;
        }

        if i != capacity {
            anyhow::bail!(
                "Capacity must be a power of 2, got {}. Try: {}",
                capacity,
                capacity.next_power_of_two()
            );
        }

        Ok(Self {
            alice_pubkey,
            bob_pubkey,
            unit,
            capacity,
            locktime,
        })
    }
}

/// Create a local mint with FakeWallet backend for testing
async fn create_local_mint(unit: CurrencyUnit) -> anyhow::Result<Mint> {
    let mint_store = Arc::new(cdk_sqlite::mint::memory::empty().await?);

    let fee_reserve = FeeReserve {
        min_fee_reserve: 1.into(),
        percent_fee_reserve: 1.0,
    };

    let fake_ln = FakeWallet::new(
        fee_reserve,
        HashMap::default(),
        HashSet::default(),
        2,
        unit.clone(),
    );

    let mut mint_builder = MintBuilder::new(mint_store.clone());
    mint_builder
        .add_payment_processor(
            unit,
            PaymentMethod::Bolt11,
            MintMeltLimits::new(1, 2_000_000_000),  // 2B msat = 2M sat
            Arc::new(fake_ln),
        )
        .await?;

    let mnemonic = Mnemonic::generate(12)?;
    mint_builder = mint_builder
        .with_name("local test mint".to_string())
        .with_urls(vec!["http://localhost:8080".to_string()]);

    let mint = mint_builder
        .build_with_seed(mint_store, &mnemonic.to_seed_normalized(""))
        .await?;

    mint.set_quote_ttl(QuoteTTL::new(10000, 10000)).await?;
    mint.start().await?;

    Ok(mint)
}

/// Direct in-process connection to a mint (no HTTP)
#[derive(Clone)]
struct DirectMintConnection {
    mint: Mint,
    auth_wallet: Arc<RwLock<Option<AuthWallet>>>,
}

impl DirectMintConnection {
    fn new(mint: Mint) -> Self {
        Self {
            mint,
            auth_wallet: Arc::new(RwLock::new(None)),
        }
    }
}

impl Debug for DirectMintConnection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DirectMintConnection")
    }
}

#[async_trait]
impl MintConnector for DirectMintConnection {
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        panic!("Not implemented");
    }

    async fn get_mint_keys(&self) -> Result<Vec<KeySet>, Error> {
        Ok(self.mint.pubkeys().keysets)
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<KeySet, Error> {
        self.mint.keyset(&keyset_id).ok_or(Error::UnknownKeySet)
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        Ok(self.mint.keysets())
    }

    async fn post_mint_quote(
        &self,
        request: MintQuoteBolt11Request,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        self.mint
            .get_mint_quote(request.into())
            .await
            .map(Into::into)
    }

    async fn get_mint_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt11Response<String>, Error> {
        self.mint
            .check_mint_quote(&QuoteId::from_str(quote_id)?)
            .await
            .map(Into::into)
    }

    async fn post_mint(&self, request: MintRequest<String>) -> Result<MintResponse, Error> {
        let request_id: MintRequest<QuoteId> = request.try_into().unwrap();
        self.mint.process_mint_request(request_id).await
    }

    async fn post_melt_quote(
        &self,
        request: MeltQuoteBolt11Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .get_melt_quote(request.into())
            .await
            .map(Into::into)
    }

    async fn get_melt_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .check_melt_quote(&QuoteId::from_str(quote_id)?)
            .await
            .map(Into::into)
    }

    async fn post_melt(
        &self,
        request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        let request_uuid = request.try_into().unwrap();
        self.mint.melt(&request_uuid).await.map(Into::into)
    }

    async fn post_swap(&self, swap_request: SwapRequest) -> Result<SwapResponse, Error> {
        self.mint.process_swap_request(swap_request).await
    }

    async fn get_mint_info(&self) -> Result<MintInfo, Error> {
        Ok(self.mint.mint_info().await?.clone().time(unix_time()))
    }

    async fn post_check_state(
        &self,
        request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.mint.check_state(&request).await
    }

    async fn post_restore(&self, request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.mint.restore(request).await
    }

    async fn get_auth_wallet(&self) -> Option<AuthWallet> {
        self.auth_wallet.read().await.clone()
    }

    async fn set_auth_wallet(&self, wallet: Option<AuthWallet>) {
        let mut auth_wallet = self.auth_wallet.write().await;
        *auth_wallet = wallet;
    }

    async fn post_mint_bolt12_quote(
        &self,
        request: MintQuoteBolt12Request,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let res: MintQuoteBolt12Response<QuoteId> =
            self.mint.get_mint_quote(request.into()).await?.try_into()?;
        Ok(res.into())
    }

    async fn get_mint_quote_bolt12_status(
        &self,
        quote_id: &str,
    ) -> Result<MintQuoteBolt12Response<String>, Error> {
        let quote: MintQuoteBolt12Response<QuoteId> = self
            .mint
            .check_mint_quote(&QuoteId::from_str(quote_id)?)
            .await?
            .try_into()?;
        Ok(quote.into())
    }

    async fn post_melt_bolt12_quote(
        &self,
        request: MeltQuoteBolt12Request,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .get_melt_quote(request.into())
            .await
            .map(Into::into)
    }

    async fn get_melt_bolt12_quote_status(
        &self,
        quote_id: &str,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        self.mint
            .check_melt_quote(&QuoteId::from_str(quote_id)?)
            .await
            .map(Into::into)
    }

    async fn post_melt_bolt12(
        &self,
        _request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        Err(Error::UnsupportedPaymentMethod)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. GENERATE KEYS FOR ALICE AND BOB
    println!("🔑 Generating keypairs...");
    let alice_secret = SecretKey::generate();
    let alice_pubkey = alice_secret.public_key();
    println!("   Alice pubkey: {}", alice_pubkey);

    let bob_secret = SecretKey::generate();
    let bob_pubkey = bob_secret.public_key();
    println!("   Bob pubkey:   {}\n", bob_pubkey);

    // 2. CREATE SPILLMAN CHANNEL PARAMETERS
    println!("📋 Setting up Spillman channel parameters...");
    let channel_params = SpillmanChannelParameters::new(
        alice_pubkey,
        bob_pubkey,
        CurrencyUnit::Msat,
        1_073_741_824,              // 2^30 msat capacity (~1,073,741 sat)
        unix_time() + 86400,        // 1 day locktime
    )?;
    println!("   Capacity: {} {:?} (2^30)", channel_params.capacity, channel_params.unit);
    println!("   Locktime: {} (1 day from now)\n", channel_params.locktime);

    // 3. CREATE LOCAL MINT
    println!("🏦 Setting up local mint...");
    let mint = create_local_mint(channel_params.unit.clone()).await?;
    println!("✅ Mint running\n");

    // 4. CREATE ALICE'S WALLET
    println!("👩 Setting up Alice's wallet...");
    let connector = DirectMintConnection::new(mint.clone());
    let alice_store = Arc::new(cdk_sqlite::wallet::memory::empty().await?);
    let alice_seed = Mnemonic::generate(12)?.to_seed_normalized("");

    let alice_wallet = WalletBuilder::new()
        .mint_url("http://localhost:8080".parse().unwrap())
        .unit(channel_params.unit.clone())  // Use channel unit
        .localstore(alice_store)
        .seed(alice_seed)
        .client(connector.clone())
        .build()?;

    // 5. MINT TOKENS FOR ALICE
    println!("💰 Alice minting {} msat (2^30)...", channel_params.capacity);
    let quote = alice_wallet.mint_quote(channel_params.capacity.into(), None).await?;
    let mut proof_stream = alice_wallet.proof_stream(quote, Default::default(), None);
    let _proofs = proof_stream.next().await.expect("proofs")?;
    println!(
        "✅ Alice has {} msat\n",
        alice_wallet.total_balance().await?
    );

    // 6. CREATE 2-OF-2 MULTISIG SPENDING CONDITIONS
    println!("🔒 Creating 2-of-2 multisig token...");
    let conditions = Conditions::new(
        Some(channel_params.locktime),     // locktime for refunds
        Some(vec![channel_params.bob_pubkey]),    // Bob's key as additional pubkey
        None,                              // no refund keys (for now)
        Some(2),                           // require 2 signatures
        Some(SigFlag::SigInputs),         // default sig flag
        None,                              // no refund sigs
    )?;

    let spending_conditions = SpendingConditions::new_p2pk(
        channel_params.alice_pubkey, // Alice's key as primary
        Some(conditions),
    );

    println!("   Requires signatures from BOTH Alice and Bob");

    // 7. ALICE CREATES LOCKED TOKEN
    let send_amount = channel_params.capacity / 2;  // Half the capacity
    let prepared = alice_wallet
        .prepare_send(
            send_amount.into(),
            SendOptions {
                conditions: Some(spending_conditions),
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;

    let token = prepared.confirm(None).await?;
    println!("✅ Token created: {} msat (2^29) locked to 2-of-2 multisig", send_amount);
    println!(
        "   Alice balance: {} msat\n",
        alice_wallet.total_balance().await?
    );

    // 8. CREATE BOB'S WALLET
    println!("👨 Setting up Bob's wallet...");
    let bob_connector = DirectMintConnection::new(mint.clone());
    let bob_store = Arc::new(cdk_sqlite::wallet::memory::empty().await?);
    let bob_seed = Mnemonic::generate(12)?.to_seed_normalized("");

    let bob_wallet = WalletBuilder::new()
        .mint_url("http://localhost:8080".parse().unwrap())
        .unit(channel_params.unit.clone())  // Use channel unit
        .localstore(bob_store)
        .seed(bob_seed)
        .client(bob_connector)
        .build()?;

    // 9. COLLABORATIVE REDEEM - BOTH ALICE AND BOB SIGN
    println!("🤝 Redeeming with BOTH signatures...");
    let received = bob_wallet
        .receive(
            &token.to_string(),
            ReceiveOptions {
                p2pk_signing_keys: vec![alice_secret.clone(), bob_secret], // Both keys!
                ..Default::default()
            },
        )
        .await?;

    println!("✅ Redeemed {} msat!", u64::from(received));
    println!(
        "   Bob balance: {} msat\n",
        bob_wallet.total_balance().await?
    );

    // 10. TRY WITH ONLY ONE KEY (WILL FAIL)
    println!("❌ Testing with only Alice's signature...");

    // Create another locked token
    let spending_conditions2 = SpendingConditions::new_p2pk(
        channel_params.alice_pubkey,
        Some(Conditions::new(
            Some(channel_params.locktime),
            Some(vec![channel_params.bob_pubkey]),
            None,
            Some(2),
            Some(SigFlag::SigInputs),
            None,
        )?),
    );

    let test_amount = channel_params.capacity / 4;  // Quarter of capacity
    let prepared2 = alice_wallet
        .prepare_send(
            test_amount.into(),
            SendOptions {
                conditions: Some(spending_conditions2),
                include_fee: true,
                ..Default::default()
            },
        )
        .await?;

    let token2 = prepared2.confirm(None).await?;

    // Try to redeem with only Alice's key
    let result = bob_wallet
        .receive(
            &token2.to_string(),
            ReceiveOptions {
                p2pk_signing_keys: vec![alice_secret], // Only one key!
                ..Default::default()
            },
        )
        .await;

    match result {
        Ok(_) => println!("   Unexpected: succeeded with 1 signature"),
        Err(e) => println!("   ✅ Correctly failed: {}\n", e),
    }

    println!("🎉 Demo complete!");
    println!("   2-of-2 multisig works!");
    println!("   Both signatures are required to spend.");

    Ok(())
}
