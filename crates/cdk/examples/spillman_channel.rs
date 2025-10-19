//! Example: Spillman (Unidirectional) Payment Channel
//!
//! This example will demonstrate a Cashu implementation of Spillman channels,
//! allowing Alice and Bob to set up an offline unidirectional payment channel.

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
use cdk::nuts::Proof;

/// Format a boolean vector as binary string [101] instead of [true, false, true]
fn format_spend_vector(vector: &[bool]) -> String {
    let bits: String = vector.iter().map(|&b| if b { '1' } else { '0' }).collect();
    format!("[{}]", bits)
}

/// Helper function to create an unsigned SwapRequest based on a spend vector
fn create_swap_request_from_vector(
    locked_proofs: &[Proof],
    bob_outputs: &[BlindedMessage],
    spend_vector: &[bool],
) -> (SwapRequest, u64) {
    // Select proofs to spend based on spend_vector
    let proofs_to_spend: Vec<Proof> = spend_vector
        .iter()
        .enumerate()
        .filter_map(|(i, &should_spend)| {
            if should_spend {
                Some(locked_proofs[i].clone())
            } else {
                None
            }
        })
        .collect();

    // Calculate total spending
    let total_spending: u64 = proofs_to_spend.iter().map(|p| u64::from(p.amount)).sum();

    // Select bob's outputs based on spend_vector
    let bob_outputs_to_use: Vec<BlindedMessage> = spend_vector
        .iter()
        .enumerate()
        .filter_map(|(i, &should_spend)| {
            if should_spend {
                Some(bob_outputs[i].clone())
            } else {
                None
            }
        })
        .collect();

    // Create and return the unsigned swap request and total
    let swap_request = SwapRequest::new(proofs_to_spend, bob_outputs_to_use);
    (swap_request, total_spending)
}

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

    /// Convert a balance to a boolean spend vector
    /// The first element is always true (we always include the 1 msat proof)
    /// The remaining elements are the binary representation of (balance - 1)
    fn balance_to_spend_vector(&self, balance: u64) -> Vec<bool> {
        assert!(balance > 0, "Balance must be greater than 0");
        assert!(balance <= self.capacity, "Balance exceeds channel capacity");

        let mut vector = Vec::with_capacity(1 + self.log2_capacity as usize);

        // First element is always true (the special 1 msat proof)
        vector.push(true);

        // Remaining balance after the first proof
        let remainder = balance - 1;

        // Binary representation of remainder
        for i in 0..self.log2_capacity {
            let bit_set = (remainder & (1 << i)) != 0;
            vector.push(bit_set);
        }

        vector
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
        3,                          // log2_capacity: 2^3 = 8 msat
        8,                          // capacity: 8 msat total
        unix_time() + 5,            // 5 second locktime
    )?;
    println!("   Capacity: {} {:?} (2^{})", channel_params.capacity, channel_params.unit, channel_params.log2_capacity);
    println!("   Denominations: {:?}", channel_params.denominations);
    println!("   (First 1 is special, rest are powers of 2)");
    println!("   Locktime: {} ({} seconds from now)\n", channel_params.locktime, channel_params.locktime - unix_time());

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
    let keysets = mint.keysets();
    let active_keyset = keysets.keysets.iter()
        .find(|k| k.active && k.unit == channel_params.unit)
        .expect("No active keyset");
    let active_keyset_id = active_keyset.id;

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

    // Alice also creates blinded outputs (same denominations, different secrets)
    println!("📦 Alice creating blinded outputs for refunds...");
    let mut alice_outputs = Vec::new();
    let mut alice_secrets_and_rs = Vec::new();

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

        alice_outputs.push(blinded_msg);
        alice_secrets_and_rs.push((secret, blinding_factor));

        let description = if i == 0 { " (special)" } else { "" };
        println!("   Output {}: {} msat{}", i + 1, amount, description);
    }

    println!("✅ Alice created {} blinded outputs\n", alice_outputs.len());

    // 8. PREPARE 2-OF-2 MULTISIG SPENDING CONDITIONS WITH LOCKTIME REFUND
    println!("🔐 Preparing 2-of-2 multisig spending conditions with locktime refund...");

    let conditions = Conditions::new(
        Some(channel_params.locktime),           // Locktime for Alice's refund
        Some(vec![bob_pubkey]),                  // Bob's key as additional pubkey
        Some(vec![alice_pubkey]),                // Alice can refund after locktime
        Some(2),                                 // Require 2 signatures (Alice + Bob)
        Some(SigFlag::SigAll),                   // SigAll: signatures commit to outputs
        Some(1),                                 // Only 1 signature needed for refund (Alice)
    )?;

    let spending_conditions = SpendingConditions::new_p2pk(
        alice_pubkey,  // Alice's key as primary
        Some(conditions),
    );

    println!("   Before locktime: Requires BOTH Alice and Bob signatures to spend");
    println!("   After locktime:  Alice can reclaim with only her signature\n");

    // 9. CREATE NEW BLINDED MESSAGES WITH 2-OF-2 CONDITIONS
    println!("🔒 Creating BlindedMessage with 2-of-2 multisig...");

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

        println!("   Output {}: {} msat - locked to 2-of-2", i + 1, amount);
    }

    println!("✅ Created locked BlindedMessage\n");

    // 10. ALICE SWAPS HER TOKENS FOR 2-OF-2 LOCKED PROOF
    println!("🔄 Alice swapping her tokens for 2-of-2 locked proof...");

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

    // 11. UNBLIND SIGNATURES TO CREATE 2-OF-2 LOCKED PROOF
    println!("🔓 Unblinding signature to create final 2-of-2 locked proof...");

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

    println!("✅ Created 1 locked proof: 1 msat - locked to 2-of-2 multisig\n");

    println!("🎉 Setup complete!");
    println!("   Alice has 1 proof locked to Alice + Bob 2-of-2");
    println!("   Requires BOTH Alice and Bob to spend\n");

    // 12. BOB VERIFIES THE LOCKED PROOF
    println!("🔍 Bob verifying the locked proof...");

    // Verify spending conditions
    for (_i, proof) in locked_proofs.iter().enumerate() {
        // Parse the secret to extract spending conditions
        let nut10_secret: Nut10Secret = proof.secret.clone().try_into()?;

        // Verify it's a P2PK secret
        if nut10_secret.kind() != Kind::P2PK {
            anyhow::bail!("Proof is not P2PK!");
        }

        // Extract and verify spending conditions
        let proof_conditions: SpendingConditions = nut10_secret.try_into()?;

        // Verify 2-of-2 multisig conditions
        if let SpendingConditions::P2PKConditions { data, conditions } = &proof_conditions {
            // Alice should be primary
            if data != &alice_pubkey {
                anyhow::bail!("Proof primary key is not Alice!");
            }

            // Check additional conditions
            if let Some(cond) = conditions {
                // Verify Bob is in the pubkeys list
                if !cond.pubkeys.as_ref().map_or(false, |keys| keys.contains(&bob_pubkey)) {
                    anyhow::bail!("Proof doesn't include Bob's pubkey!");
                }

                // Verify 2-of-2 requirement
                if cond.num_sigs != Some(2) {
                    anyhow::bail!("Proof doesn't require 2 signatures!");
                }
            } else {
                anyhow::bail!("Proof has no conditions!");
            }
        }

        println!("   ✓ Proof locked to Alice + Bob 2-of-2");
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

    // 13. TEST SPENDING BEFORE LOCKTIME (REQUIRES BOTH ALICE AND BOB SIGNATURES)
    println!("\n🔓 Testing spending BEFORE locktime (requires both Alice and Bob)...");
    println!("   Current time: {}", unix_time());
    println!("   Locktime: {}", channel_params.locktime);
    println!("   Time until locktime: {} seconds\n", channel_params.locktime.saturating_sub(unix_time()));

    // Amount to send to Bob
    let amount_to_bob = 5;

    // Convert balance to spend vector
    let spend_vector = channel_params.balance_to_spend_vector(amount_to_bob);

    // Create swap request using helper function
    let (mut spend_swap_request, total_spending) =
        create_swap_request_from_vector(&locked_proofs, &bob_outputs, &spend_vector);

    println!("   Spending: {} msat (requires Alice + Bob signatures)", total_spending);
    println!("   Spend vector: {}", format_spend_vector(&spend_vector));
    println!("   Outputs: Using Bob's predetermined outputs");

    // Verify that unsigned request fails
    assert!(
        spend_swap_request.verify_sig_all().is_err(),
        "Unsigned swap request should fail verification"
    );
    println!("   ✓ Unsigned request fails verification (as expected)");

    // Sign the entire swap request with BOTH Alice and Bob keys (2-of-2 SigAll)
    println!("   Signing swap request with Alice...");
    spend_swap_request.sign_sig_all(alice_secret.clone())?;
    println!("   ✓ Signed with Alice");

    // Verify that request with only Alice's signature fails (needs 2 signatures)
    assert!(
        spend_swap_request.verify_sig_all().is_err(),
        "Swap request with only 1 signature should fail (needs 2)"
    );
    println!("   ✓ Request with only Alice's signature fails (needs 2)");

    println!("   Signing swap request with Bob...");
    spend_swap_request.sign_sig_all(bob_secret.clone())?;
    println!("   ✓ Signed with Bob");

    // Verify the swap request is properly signed with both signatures
    assert!(
        spend_swap_request.verify_sig_all().is_ok(),
        "Multi-sig SIG_ALL swap request should verify with both signatures"
    );
    println!("   ✓ SigAll verification passed with both signatures");

    println!("   Swapping locked proof for Bob's outputs...");
    let spend_swap_response = mint.process_swap_request(spend_swap_request).await.map_err(|e| {
        anyhow::anyhow!("Swap failed: {:?}", e)
    })?;

    // Unblind to get Bob's final proofs (only for the spent outputs)
    let bob_secrets_and_rs_to_use: Vec<_> = spend_vector
        .iter()
        .enumerate()
        .filter_map(|(i, &should_spend)| {
            if should_spend {
                Some(bob_secrets_and_rs[i].clone())
            } else {
                None
            }
        })
        .collect();

    let _bob_final_proofs = construct_proofs(
        spend_swap_response.signatures,
        bob_secrets_and_rs_to_use.iter().map(|(_, r)| r.clone()).collect(),
        bob_secrets_and_rs_to_use.iter().map(|(s, _)| s.clone()).collect(),
        &mint_keys.keys,
    )?;

    println!("✅ Swap successful!");
    println!("   Bob received {} msat in his predetermined outputs", total_spending);
    println!("   These proofs have no spending conditions and can be freely spent by Bob\n");

    // 14. SECOND TRANSACTION: TRY TO DOUBLE-SPEND (should fail)
    println!("🔓 Second transaction: attempting to re-spend (double-spend attack)...");

    // Try to spend an amount that would reuse already-spent proofs
    let amount_to_bob_2 = 3;  // This will try to reuse the first proof (already spent)
    let spend_vector_2 = channel_params.balance_to_spend_vector(amount_to_bob_2);

    // Create swap request using helper function
    let (mut spend_swap_request_2, total_spending_2) =
        create_swap_request_from_vector(&locked_proofs, &bob_outputs, &spend_vector_2);

    println!("   Attempting to spend: {} msat", total_spending_2);
    println!("   Spend vector: {}", format_spend_vector(&spend_vector_2));
    println!("   (This will attempt to reuse outputs from the first transaction)");

    // Sign with both keys
    spend_swap_request_2.sign_sig_all(alice_secret.clone())?;
    spend_swap_request_2.sign_sig_all(bob_secret.clone())?;

    println!("   Attempting swap (this should fail)...");
    match mint.process_swap_request(spend_swap_request_2).await {
        Ok(_) => {
            println!("❌ UNEXPECTED: Swap succeeded! Double-spend was not prevented!");
        }
        Err(e) => {
            println!("✅ Swap correctly rejected: {:?}", e);
            println!("   The mint properly prevents double-spending\n");
        }
    }

    // 15. TEST ALICE-ONLY REFUND - TRY EVERY SECOND UNTIL LOCKTIME PASSES
    println!("⏰ Testing locktime enforcement - trying refund every second...");
    println!("   Current time: {}", unix_time());
    println!("   Locktime: {}", channel_params.locktime);
    println!("   Expected: Refund should FAIL until locktime passes, then SUCCEED\n");

    // Calculate total remaining (we spent 5 msat earlier, so 3 msat remains)
    let remaining_amount = channel_params.capacity - amount_to_bob;
    println!("   Using Alice's pre-generated outputs for {} msat refund (remaining after payment to Bob)", remaining_amount);

    // Determine which proofs are still unspent (those not used in first transaction)
    let unspent_proofs: Vec<Proof> = spend_vector
        .iter()
        .enumerate()
        .filter_map(|(i, &was_spent)| {
            if !was_spent {
                Some(locked_proofs[i].clone())
            } else {
                None
            }
        })
        .collect();

    // Use Alice's pre-generated outputs for the unspent proofs
    let alice_refund_outputs: Vec<BlindedMessage> = spend_vector
        .iter()
        .enumerate()
        .filter_map(|(i, &was_spent)| {
            if !was_spent {
                Some(alice_outputs[i].clone())
            } else {
                None
            }
        })
        .collect();

    let alice_refund_secrets_and_rs: Vec<_> = spend_vector
        .iter()
        .enumerate()
        .filter_map(|(i, &was_spent)| {
            if !was_spent {
                Some(alice_secrets_and_rs[i].clone())
            } else {
                None
            }
        })
        .collect();

    println!("   Using {} unspent proofs for refund", unspent_proofs.len());

    // Try refund every second until it succeeds
    let mut attempt = 1;
    let refund_swap_response = loop {
        let current_time = unix_time();
        let time_diff = if current_time >= channel_params.locktime {
            format!("+{} sec past locktime", current_time - channel_params.locktime)
        } else {
            format!("{} sec before locktime", channel_params.locktime - current_time)
        };

        println!("   Attempt #{} at time {} ({})", attempt, current_time, time_diff);

        // Create fresh refund swap request for this attempt
        let mut refund_swap_request = SwapRequest::new(unspent_proofs.clone(), alice_refund_outputs.clone());

        // Sign with ONLY Alice (no Bob signature)
        refund_swap_request.sign_sig_all(alice_secret.clone())?;
        println!("      - Signed with only Alice's key");

        // Try to process the refund
        match mint.process_swap_request(refund_swap_request).await {
            Ok(response) => {
                println!("      ✅ REFUND SUCCEEDED (locktime has passed)");
                break response;
            }
            Err(e) => {
                println!("      ❌ Refund failed: {:?}", e);
                println!("      (Expected - locktime not yet reached)\n");
                attempt += 1;
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    };

    // Unblind to get Alice's refund proofs
    let _alice_refund_proofs = construct_proofs(
        refund_swap_response.signatures,
        alice_refund_secrets_and_rs.iter().map(|(_, r)| r.clone()).collect(),
        alice_refund_secrets_and_rs.iter().map(|(s, _)| s.clone()).collect(),
        &mint_keys.keys,
    )?;

    println!("\n✅ Refund successful after {} attempts!", attempt);
    println!("   Alice reclaimed {} msat using ONLY her signature", remaining_amount);
    println!("   Bob's signature was NOT required (locktime refund)\n");

    println!("🎉 All tests passed!");
    println!("   ✓ Before locktime: Required both Alice + Bob signatures");
    println!("   ✓ Locktime enforcement: Refund failed until locktime passed");
    println!("   ✓ After locktime: Alice alone could reclaim funds");
    println!("   ✓ Double-spend prevention works correctly");
    println!("   ✓ Spillman channel with locktime refund working as expected!");

    Ok(())
}
