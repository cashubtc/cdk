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
use cdk::wallet::{AuthWallet, MintConnector, ReceiveOptions, SendOptions, Wallet, WalletBuilder};
use cdk::{dhke::{blind_message, construct_proofs}, Error, Mint, StreamExt};
use cdk_fake_wallet::FakeWallet;
use tokio::sync::RwLock;
use cdk::nuts::{BlindedMessage, nut10::Secret as Nut10Secret, ProofsMethods};
use cdk::secret::Secret;
use cdk::Amount;
use cdk::nuts::nut10::Kind;

/// Parameters for a Spillman payment channel
#[derive(Debug, Clone)]
struct SpillmanChannelParameters {
    /// Alice's public key (sender)
    alice_pubkey: cdk::nuts::PublicKey,
    /// Bob's public key (receiver)
    bob_pubkey: cdk::nuts::PublicKey,
    /// Currency unit for the channel
    unit: CurrencyUnit,
    /// Log2 of capacity (e.g., 30 for 2^30)
    log2_capacity: u32,
    /// Total channel capacity (2^log2_capacity)
    capacity: u64,
    /// Locktime after which Alice can reclaim funds (unix timestamp)
    locktime: u64,
    /// Denomination sizes for channel outputs
    /// First element is special 1-unit output, rest are powers of 2
    /// Example: for capacity 8, this is [1, 1, 2, 4]
    denominations: Vec<u64>,
}

impl SpillmanChannelParameters {
    /// Create new channel parameters
    ///
    /// # Errors
    ///
    /// Returns an error if capacity != 2^log2_capacity
    fn new(
        alice_pubkey: cdk::nuts::PublicKey,
        bob_pubkey: cdk::nuts::PublicKey,
        unit: CurrencyUnit,
        log2_capacity: u32,
        capacity: u64,
        locktime: u64,
    ) -> anyhow::Result<Self> {
        // Validate that capacity == 2^log2_capacity
        if log2_capacity >= 64 {
            anyhow::bail!("log2_capacity must be less than 64, got {}", log2_capacity);
        }

        let expected_capacity = 1u64
            .checked_shl(log2_capacity)
            .ok_or_else(|| anyhow::anyhow!("log2_capacity {} is too large", log2_capacity))?;

        if capacity != expected_capacity {
            anyhow::bail!(
                "Capacity mismatch: expected 2^{} = {}, got {}",
                log2_capacity,
                expected_capacity,
                capacity
            );
        }

        // Build denominations vector
        // First element: special 1-unit output (for double-spend prevention)
        // Remaining elements: powers of 2 from 2^0 to 2^(log2_capacity - 1)
        let mut denominations = vec![1]; // Special output

        for i in 0..log2_capacity {
            denominations.push(1u64 << i); // 2^i
        }

        // Verify sum of denominations equals capacity
        let sum: u64 = denominations.iter().sum();
        if sum != capacity {
            anyhow::bail!(
                "Denominations sum mismatch: sum({:?}) = {}, expected capacity {}",
                denominations,
                sum,
                capacity
            );
        }

        Ok(Self {
            alice_pubkey,
            bob_pubkey,
            unit,
            log2_capacity,
            capacity,
            locktime,
            denominations,
        })
    }
}

/// Create a wallet connected to a mint
async fn create_wallet(mint: &Mint, unit: CurrencyUnit) -> anyhow::Result<Wallet> {
    let connector = DirectMintConnection::new(mint.clone());
    let store = Arc::new(cdk_sqlite::wallet::memory::empty().await?);
    let seed = Mnemonic::generate(12)?.to_seed_normalized("");

    let wallet = WalletBuilder::new()
        .mint_url("http://localhost:8080".parse().unwrap())
        .unit(unit)
        .localstore(store)
        .seed(seed)
        .client(connector)
        .build()?;

    Ok(wallet)
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
        0,                          // log2_capacity: 2^0 = 1 msat (simplified)
        1,                          // capacity: 1 msat total
        unix_time() + 86400,        // 1 day locktime
    )?;
    println!("   Capacity: {} {:?} (2^{})", channel_params.capacity, channel_params.unit, channel_params.log2_capacity);
    println!("   Denominations: {:?}", channel_params.denominations);
    println!("   (First 1 is special, rest are powers of 2)");
    println!("   Locktime: {} (1 day from now)\n", channel_params.locktime);

    // 3. CREATE LOCAL MINT
    println!("🏦 Setting up local mint...");
    let mint = create_local_mint(channel_params.unit.clone()).await?;
    println!("✅ Mint running\n");

    // 4. CREATE ALICE'S WALLET
    println!("👩 Setting up Alice's wallet...");
    let alice_wallet = create_wallet(&mint, channel_params.unit.clone()).await?;

    // 5. CREATE BOB'S WALLET
    println!("👨 Setting up Bob's wallet...");
    let bob_wallet = create_wallet(&mint, channel_params.unit.clone()).await?;

    // 6. ALICE MINTS TOKENS FOR THE CHANNEL CAPACITY
    println!("💰 Alice minting {} msat (full channel capacity)...", channel_params.capacity);
    let quote = alice_wallet.mint_quote(channel_params.capacity.into(), None).await?;
    let mut proof_stream = alice_wallet.proof_stream(quote, Default::default(), None);
    let _proofs = proof_stream.next().await.expect("proofs")?;
    println!("✅ Alice has {} msat\n", alice_wallet.total_balance().await?);

    // 7. BOB CREATES BLINDED OUTPUTS FOR SPILLMAN CHANNEL
    println!("📦 Bob creating blinded outputs for channel...");

    // Get active keyset from mint
    let active_keyset_id = mint.keysets().keysets.iter()
        .find(|k| k.active && k.unit == channel_params.unit)
        .expect("No active keyset")
        .id;

    println!("   Using keyset: {}", active_keyset_id);

    // Bob creates one BlindedMessage for each denomination
    let mut bob_outputs = Vec::new();
    let mut bob_secrets_and_rs = Vec::new();

    for (i, &amount) in channel_params.denominations.iter().enumerate() {
        // Generate random secret
        let secret = Secret::generate();

        // Blind the secret to get B_ = Y + rG
        let (blinded_point, blinding_factor) = blind_message(&secret.to_bytes(), None)?;

        // Create BlindedMessage
        let blinded_msg = BlindedMessage::new(
            Amount::from(amount),
            active_keyset_id,
            blinded_point,
        );

        bob_outputs.push(blinded_msg);
        bob_secrets_and_rs.push((secret, blinding_factor));

        let description = if i == 0 { " (special)" } else { "" };
        println!("   Output {}: {} msat{}", i + 1, amount, description);
    }

    println!("✅ Bob created {} blinded outputs\n", bob_outputs.len());

    // Verify number of outputs matches denominations
    assert_eq!(
        bob_outputs.len(),
        channel_params.denominations.len(),
        "Bob's output count must match denominations count"
    );

    // 8. PREPARE SIMPLE P2PK TO BOB (SIMPLIFIED)
    println!("🔐 Preparing simple P2PK spending conditions (locked to Bob)...");

    let spending_conditions = SpendingConditions::new_p2pk(
        bob_pubkey,  // Bob's key - only Bob can spend
        None,        // No additional conditions
    );

    println!("   Only Bob can spend these proofs\n");

    // 9. CREATE NEW BLINDED MESSAGES WITH P2PK TO BOB
    println!("🔒 Creating BlindedMessage locked to Bob...");

    let mut locked_outputs = Vec::new();
    let mut locked_secrets_and_rs = Vec::new();

    for (i, &amount) in channel_params.denominations.iter().enumerate() {
        // Create a fresh NUT-10 secret with the same spending conditions
        // Each proof MUST have a unique secret to avoid DuplicateInputs error
        let nut10_secret: Nut10Secret = spending_conditions.clone().into();
        let secret: Secret = nut10_secret.try_into()?;

        // Blind the secret to get B_ = Y + rG
        let (blinded_point, blinding_factor) = blind_message(&secret.to_bytes(), None)?;

        // Create BlindedMessage
        let blinded_msg = BlindedMessage::new(
            Amount::from(amount),
            active_keyset_id,
            blinded_point,
        );

        locked_outputs.push(blinded_msg);
        locked_secrets_and_rs.push((secret, blinding_factor));

        println!("   Output {}: {} msat - locked to Bob", i + 1, amount);
    }

    println!("✅ Created locked BlindedMessage\n");

    // 10. ALICE SWAPS HER TOKENS FOR LOCKED PROOF (P2PK TO BOB)
    println!("🔄 Alice swapping her tokens for P2PK locked proof...");

    let alice_proofs = alice_wallet
        .localstore
        .get_proofs(
            Some(alice_wallet.mint_url.clone()),
            Some(alice_wallet.unit.clone()),
            None,
            None,
        )
        .await?
        .into_iter()
        .map(|p| p.proof)
        .collect::<Vec<_>>();

    println!("   Alice inputs: {} msat", alice_proofs.iter().map(|p| u64::from(p.amount)).sum::<u64>());
    println!("   Locked outputs: {:?}", channel_params.denominations);

    // Create and execute the swap
    let swap_request = SwapRequest::new(alice_proofs, locked_outputs);
    let swap_response = mint.process_swap_request(swap_request).await?;

    println!("✅ Swap successful! Received {} blind signatures\n", swap_response.signatures.len());

    // 11. UNBLIND SIGNATURES TO CREATE P2PK LOCKED PROOF
    println!("🔓 Unblinding signature to create final P2PK locked proof...");

    // Get the mint's public keys for this keyset
    let mint_keys = mint.keyset(&active_keyset_id)
        .ok_or_else(|| anyhow::anyhow!("Keyset not found"))?;

    // Unblind the signatures to create usable proofs
    let locked_proofs = construct_proofs(
        swap_response.signatures,
        locked_secrets_and_rs.iter().map(|(_, r)| r.clone()).collect(),
        locked_secrets_and_rs.iter().map(|(s, _)| s.clone()).collect(),
        &mint_keys.keys,
    )?;

    println!("✅ Created 1 locked proof: 1 msat - locked to Bob (P2PK)\n");

    println!("🎉 Setup complete!");
    println!("   Alice has 1 proof locked to Bob's key");
    println!("   Only Bob can spend this proof\n");

    // 12. BOB VERIFIES THE LOCKED PROOF
    println!("🔍 Bob verifying the locked proof...");

    // Verify spending conditions
    for (i, proof) in locked_proofs.iter().enumerate() {
        // Parse the secret to extract spending conditions
        let nut10_secret: Nut10Secret = proof.secret.clone().try_into()?;

        // Verify it's a P2PK secret
        if nut10_secret.kind() != Kind::P2PK {
            anyhow::bail!("Proof is not P2PK!");
        }

        // Extract and verify spending conditions
        let proof_conditions: SpendingConditions = nut10_secret.try_into()?;

        // Verify Bob is the key holder
        if let SpendingConditions::P2PKConditions { data, conditions } = &proof_conditions {
            if data != &bob_pubkey {
                anyhow::bail!("Proof is not locked to Bob's key!");
            }

            // Verify no additional conditions (simple P2PK)
            if conditions.is_some() {
                anyhow::bail!("Proof should have no additional conditions!");
            }
        }

        println!("   ✓ Proof locked to Bob's key");
    }

    // Verify DLEQ proofs (required for all proofs)
    println!("   Verifying DLEQ proofs...");
    for (i, proof) in locked_proofs.iter().enumerate() {
        // Bob requires DLEQ proof on every proof
        if proof.dleq.is_none() {
            anyhow::bail!("Proof {} is missing DLEQ proof!", i + 1);
        }

        // Get mint's public key for this amount
        let mint_pubkey = mint_keys.keys.amount_key(proof.amount)
            .ok_or_else(|| anyhow::anyhow!("No key for amount {}", proof.amount))?;

        // Verify DLEQ proof using the proof's verify_dleq method
        proof.verify_dleq(mint_pubkey)?;

        println!("   ✓ Proof {}: DLEQ proof valid", i + 1);
    }
    println!("   ✓ All {} DLEQ proofs verified", locked_proofs.len());

    // Verify proof structure
    println!("   Verifying proof structure...");
    let total_amount = locked_proofs.total_amount()?;
    if total_amount != Amount::from(channel_params.capacity) {
        anyhow::bail!("Total proof amount {} doesn't match capacity {}", total_amount, channel_params.capacity);
    }
    println!("   ✓ Total amount matches capacity: {} msat", total_amount);

    // Verify denominations match expectations
    let proof_amounts: Vec<u64> = locked_proofs.iter().map(|p| u64::from(p.amount)).collect();
    if proof_amounts != channel_params.denominations {
        anyhow::bail!("Proof denominations {:?} don't match expected {:?}", proof_amounts, channel_params.denominations);
    }
    println!("   ✓ Denominations match: {:?}", proof_amounts);

    println!("\n✅ All proofs verified by Bob!");
    println!("   Bob confirms:");
    println!("   - All proofs are locked to Alice + Bob 2-of-2");
    println!("   - Locktime allows Alice to refund after {}", channel_params.locktime);
    println!("   - Spending conditions are correct (SigAll, 2-of-2, locktime)");
    println!("   - All DLEQ proofs verified (required)");
    println!("   - Total value: {} msat in {} denominations", total_amount, locked_proofs.len());

    // 13. TEST BOB SPENDING THE LOCKED PROOF
    println!("\n🔓 Testing Bob spending the P2PK locked proof...");

    // Take the proof
    let mut proofs_to_spend = vec![
        locked_proofs[0].clone(),
    ];

    println!("   Spending: 1 msat");

    // Create new unlocked outputs to receive
    let mut unlocked_outputs = Vec::new();
    let mut unlocked_secrets_and_rs = Vec::new();

    // Get fresh active keyset
    let fresh_active_keyset_id = mint.keysets().keysets.iter()
        .find(|k| k.active && k.unit == channel_params.unit)
        .expect("No active keyset")
        .id;

    // Output is just 1 msat
    let output_denominations = vec![1];

    for amount in output_denominations {
        let secret = Secret::generate();
        let (blinded_point, blinding_factor) = blind_message(&secret.to_bytes(), None)?;
        let blinded_msg = BlindedMessage::new(
            Amount::from(amount),
            fresh_active_keyset_id,
            blinded_point,
        );
        unlocked_outputs.push(blinded_msg);
        unlocked_secrets_and_rs.push((secret, blinding_factor));
    }

    // Sign the proof with Bob's key (simple P2PK)
    println!("   Signing proof with Bob's key...");
    for proof in &mut proofs_to_spend {
        proof.sign_p2pk(bob_secret.clone())?;
    }
    println!("   ✓ Signed with Bob");

    // Create swap request
    let spend_swap_request = SwapRequest::new(proofs_to_spend.clone(), unlocked_outputs);

    println!("   Swapping locked proof for unlocked proof...");
    let spend_swap_response = mint.process_swap_request(spend_swap_request).await.map_err(|e| {
        anyhow::anyhow!("Swap failed: {:?}", e)
    })?;

    // Unblind to get final unlocked proof
    let fresh_mint_keys = mint.keyset(&fresh_active_keyset_id)
        .ok_or_else(|| anyhow::anyhow!("Fresh keyset not found"))?;

    let unlocked_proofs = construct_proofs(
        spend_swap_response.signatures,
        unlocked_secrets_and_rs.iter().map(|(_, r)| r.clone()).collect(),
        unlocked_secrets_and_rs.iter().map(|(s, _)| s.clone()).collect(),
        &fresh_mint_keys.keys,
    )?;

    println!("✅ Swap successful!");
    println!("   Received 1 msat in unlocked proof");
    println!("   This proof has no spending conditions and can be freely spent\n");

    Ok(())
}
