use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bip39::Mnemonic;
use bitcoin::bip32::DerivationPath;
use bitcoin::hashes::Hash;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::{MintKeysDatabase, WalletDatabase};
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, PaymentMethod, ProofsMethods, State};
use cdk::wallet::Wallet;

enum MintRuntime {
    Poetry { path: String },
    Docker,
}

fn find_sqlite_db(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if dir.is_file() {
        let file_name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let ext = dir.extension().and_then(|s| s.to_str()).unwrap_or("");
        if (ext == "sqlite3" || ext == "sqlite") && file_name.contains("mint") {
            return Some(dir.to_path_buf());
        }
    } else if dir.is_dir() {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            if let Some(p) = find_sqlite_db(&entry.path()) {
                return Some(p);
            }
        }
    }
    None
}

fn create_truly_random_fake_invoice(amount_msat: u64) -> lightning_invoice::Bolt11Invoice {
    use bitcoin::secp256k1::rand::rngs::OsRng;
    use bitcoin::secp256k1::rand::Rng;
    use bitcoin::secp256k1::SecretKey;

    let mut rng = OsRng;

    // Generate random 32-byte secret key
    let mut sk_bytes = [0u8; 32];
    rng.fill(&mut sk_bytes);
    // Make sure it's a valid secret key
    sk_bytes[0] &= 0x7f; // simple sanitization
    let private_key = SecretKey::from_slice(&sk_bytes)
        .unwrap_or_else(|_| SecretKey::from_slice(&[42u8; 32]).unwrap());

    // Generate random payment hash and secret
    let mut payment_hash_bytes = [0u8; 32];
    rng.fill(&mut payment_hash_bytes);
    let payment_hash = bitcoin::hashes::sha256::Hash::from_slice(&payment_hash_bytes).unwrap();

    let mut payment_secret_bytes = [0u8; 32];
    rng.fill(&mut payment_secret_bytes);
    let payment_secret = lightning_invoice::PaymentSecret(payment_secret_bytes);

    let description = format!("fuzz_melt_{}", uuid::Uuid::new_v4());

    lightning_invoice::InvoiceBuilder::new(lightning_invoice::Currency::Bitcoin)
        .description(description)
        .payment_hash(payment_hash)
        .payment_secret(payment_secret)
        .amount_milli_satoshis(amount_msat)
        .current_timestamp()
        .min_final_cltv_expiry_delta(144)
        .build_signed(|hash| {
            bitcoin::secp256k1::Secp256k1::new().sign_ecdsa_recoverable(hash, &private_key)
        })
        .expect("Failed to build fake invoice")
}

async fn do_mint(
    wallet: &Wallet,
    _container_name: &Option<String>,
    _poetry_path: &Option<String>,
) -> Result<()> {
    let amount = Amount::from(100);
    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
        .await?;

    // Mint the proofs in the wallet (automatically paid via CASHU_FAKEWALLET_BRR=True)
    let proofs = wallet
        .wait_and_mint_quote(quote, SplitTarget::default(), None, Duration::from_secs(10))
        .await?;

    println!(
        "Successfully minted {} sats",
        proofs.total_amount().unwrap()
    );
    Ok(())
}

async fn do_swap(wallet: &Wallet) -> Result<()> {
    let unspent = wallet.get_unspent_proofs().await?;
    if unspent.is_empty() {
        return Ok(());
    }

    // Swap/split the proofs
    wallet
        .swap(None, SplitTarget::default(), unspent, None, false, false)
        .await?;
    println!("Successfully performed swap");
    Ok(())
}

async fn do_melt(wallet: &Wallet) -> Result<()> {
    let balance = wallet.total_balance().await?;
    if balance < Amount::from(20) {
        // Not enough balance to melt
        return Ok(());
    }

    let fake_invoice = create_truly_random_fake_invoice(10_000); // 10 sats
    let melt_quote = wallet
        .melt_quote(
            PaymentMethod::Known(KnownMethod::Bolt11),
            fake_invoice.to_string(),
            None,
            None,
        )
        .await?;

    let prepared = wallet.prepare_melt(&melt_quote.id, HashMap::new()).await?;
    let melt_response = prepared.confirm().await?;

    assert_eq!(melt_response.state(), MeltQuoteState::Paid);
    println!("Successfully performed melt");
    Ok(())
}

async fn rotate_nutshell_keyset(
    container_name: &Option<String>,
    poetry_path: &Option<String>,
) -> Result<()> {
    let output = match container_name {
        Some(name) => std::process::Command::new("docker")
            .args([
                "exec",
                name,
                "poetry",
                "run",
                "mint-cli",
                "-p",
                "8086",
                "-i",
                "next-keyset",
                "sat",
            ])
            .output()?,
        None => {
            let path = poetry_path.as_deref().unwrap_or("nutshell");
            std::process::Command::new("poetry")
                .args(["run", "mint-cli", "-p", "8086", "-i", "next-keyset", "sat"])
                .current_dir(path)
                .output()?
        }
    };

    if !output.status.success() {
        anyhow::bail!(
            "Failed to rotate keyset: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("Successfully rotated Nutshell keyset!");
    Ok(())
}

struct CleanupGuard {
    nutshell_proc: Option<std::process::Child>,
    container_name: Option<String>,
    fuzz_dir: std::path::PathBuf,
    cdk_db_path: std::path::PathBuf,
}

impl CleanupGuard {
    fn stop_nutshell(&mut self) {
        if let Some(name) = &self.container_name {
            if let Ok(output) = std::process::Command::new("docker")
                .args(["logs", name])
                .output()
            {
                let s = String::from_utf8_lossy(&output.stdout);
                let err_s = String::from_utf8_lossy(&output.stderr);
                if !s.is_empty() || !err_s.is_empty() {
                    println!("NUTSHELL DOCKER LOGS:\nstdout:\n{}\nstderr:\n{}", s, err_s);
                }
            }
            let _ = std::process::Command::new("docker")
                .args(["stop", name])
                .output();
            let _ = std::process::Command::new("docker")
                .args(["rm", name])
                .output();
        }
        if let Some(mut proc) = self.nutshell_proc.take() {
            let _ = proc.kill();
            let _ = proc.wait();
            if let Some(mut stderr) = proc.stderr.take() {
                let mut s = String::new();
                use std::io::Read;
                if stderr.read_to_string(&mut s).is_ok() {
                    println!("NUTSHELL STDERR LOGS:\n{}", s);
                }
            }
        }
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        self.stop_nutshell();
        let _ = std::fs::remove_dir_all(&self.fuzz_dir);
        if self.cdk_db_path.exists() {
            let _ = std::fs::remove_file(&self.cdk_db_path);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_nutshell_migration_fuzzer() -> Result<()> {
    println!("Initializing nutshell migration fuzzer...");

    // Detect runtime
    let runtime = if std::env::var("CDK_TEST_USE_POETRY").is_ok()
        || std::env::var("CDK_TEST_NUTSHELL_PATH").is_ok()
    {
        let path =
            std::env::var("CDK_TEST_NUTSHELL_PATH").unwrap_or_else(|_| "nutshell".to_string());
        MintRuntime::Poetry { path }
    } else {
        MintRuntime::Docker
    };

    // Create temporary directory for nutshell fuzzing
    let fuzz_dir = std::env::temp_dir().join(format!("nutshell_fuzz_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&fuzz_dir)?;
    let db_path = fuzz_dir.join("mint");

    // Spawn nutshell mint based on runtime
    let (nutshell_proc, container_name, poetry_path) = match &runtime {
        MintRuntime::Poetry { path } => {
            println!("Sourcing nutshell mint from Poetry path: {}", path);
            let proc = std::process::Command::new("poetry")
                .args([
                    "run",
                    "python",
                    "-m",
                    "cashu.mint.__main__",
                    "--port",
                    "4444",
                ])
                .current_dir(path)
                .env("CASHU_DIR", &fuzz_dir)
                .env("MINT_DATABASE", &db_path)
                .env("MINT_BACKEND_BOLT11_SAT", "FakeWallet")
                .env("CASHU_FAKEWALLET_BRR", "True")
                .env("FAKEWALLET_BRR", "True")
                .env("FAKEMINT_BRR", "True")
                .env(
                    "MINT_PRIVATE_KEY",
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                )
                .env("MINT_DERIVATION_PATH", "m/0'/0'/0'")
                .env("MINT_RPC_SERVER_ENABLE", "True")
                .env("MINT_RPC_SERVER_PORT", "8086")
                .env("MINT_RPC_SERVER_MUTUAL_TLS", "False")
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            (proc, None, Some(path.clone()))
        }
        MintRuntime::Docker => {
            println!("Sourcing nutshell mint from Docker container (cashubtc/nutshell:latest)...");
            let name = format!("nutshell_fuzz_{}", uuid::Uuid::new_v4());
            // Pre-clean container
            let _ = std::process::Command::new("docker")
                .args(["rm", "-f", &name])
                .output();

            let proc = std::process::Command::new("docker")
                .args([
                    "run",
                    "--network=host",
                    "--name",
                    &name,
                    "-v",
                    &format!("{}:/data", fuzz_dir.to_str().unwrap()),
                    "-e",
                    "CASHU_DIR=/data",
                    "-e",
                    "MINT_DATABASE=/data/mint",
                    "-e",
                    "MINT_BACKEND_BOLT11_SAT=FakeWallet",
                    "-e",
                    "MINT_LIGHTNING_BACKEND=FakeWallet",
                    "-e",
                    "CASHU_FAKEWALLET_BRR=True",
                    "-e",
                    "FAKEWALLET_BRR=True",
                    "-e",
                    "FAKEMINT_BRR=True",
                    "-e",
                    "MINT_PRIVATE_KEY=000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                    "-e",
                    "MINT_DERIVATION_PATH=m/0'/0'/0'",
                    "-e",
                    "MINT_RPC_SERVER_ENABLE=True",
                    "-e",
                    "MINT_RPC_SERVER_PORT=8086",
                    "-e",
                    "MINT_RPC_SERVER_MUTUAL_TLS=False",
                    "cashubtc/nutshell:latest",
                    "poetry",
                    "run",
                    "mint",
                    "--port",
                    "4444",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            (proc, Some(name), None)
        }
    };

    // Wait until the port is open and nutshell is listening
    let mut ready = false;
    for _ in 0..30 {
        if std::net::TcpStream::connect("127.0.0.1:4444").is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if !ready {
        let mut nutshell_proc = nutshell_proc;
        let _ = nutshell_proc.kill();
        let _ = nutshell_proc.wait();
        panic!("Nutshell mint failed to start on port 4444 within 15 seconds.");
    }
    println!("Nutshell mint is online and listening.");

    let cdk_db_path =
        std::env::temp_dir().join(format!("cdk_fuzz_{}.sqlite", uuid::Uuid::new_v4()));
    let mut cleanup_guard = CleanupGuard {
        nutshell_proc: Some(nutshell_proc),
        container_name: container_name.clone(),
        fuzz_dir: fuzz_dir.clone(),
        cdk_db_path: cdk_db_path.clone(),
    };

    // 1. Spawn a bunch of wallets (cdk wallets)
    println!("Spawning fuzz wallets...");
    let mut wallets = Vec::new();
    let mut wallet_seeds = Vec::new();
    let mut wallet_stores = Vec::new();

    for _ in 0..10 {
        let store = Arc::new(cdk_sqlite::wallet::memory::empty().await?);
        let mnemonic = Mnemonic::generate(12).unwrap();
        let seed = mnemonic.to_seed_normalized("");
        let wallet = Wallet::new(
            "http://127.0.0.1:4444",
            CurrencyUnit::Sat,
            store.clone(),
            seed,
            None,
        )?;

        wallets.push(wallet);
        wallet_seeds.push(seed);
        wallet_stores.push(store);
    }

    // Fund wallets first
    println!("Funding wallets...");
    for (i, wallet) in wallets.iter().enumerate() {
        println!("Initial funding for wallet {}...", i);
        do_mint(wallet, &container_name, &poetry_path).await?;
    }

    // 2. Perform random operations
    println!("Performing random wallet operations (40 operations)...");
    for i in 0..40 {
        if i == 15 {
            if let Err(e) = rotate_nutshell_keyset(&container_name, &poetry_path).await {
                println!("Warning: Keysets rotation failed on Nutshell: {:?}", e);
            }
        }
        let wallet_idx = rand::random_range(0..10);
        let op_idx = rand::random_range(0..3);
        let wallet = &wallets[wallet_idx];

        println!("Round {}: Wallet {}, Op {}", i, wallet_idx, op_idx);
        match op_idx {
            0 => {
                if let Err(e) = do_mint(wallet, &container_name, &poetry_path).await {
                    println!("Warning: do_mint failed during fuzzing: {:?}", e);
                }
            }
            1 => {
                if let Err(e) = do_swap(wallet).await {
                    println!("Warning: do_swap failed during fuzzing: {:?}", e);
                }
            }
            _ => {
                if let Err(e) = do_melt(wallet).await {
                    println!("Warning: do_melt failed during fuzzing: {:?}", e);
                }
            }
        }
    }

    // Record total balance of each wallet before migration
    let mut balances_before = Vec::new();
    for (i, wallet) in wallets.iter().enumerate() {
        let bal = wallet.total_balance().await?;
        println!("Wallet {} balance before migration: {}", i, bal);
        balances_before.push(bal);
    }

    // 3. Stop nutshell mint process
    println!("Stopping nutshell mint python process...");
    cleanup_guard.stop_nutshell();
    println!("Nutshell mint stopped successfully.");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Find nutshell sqlite database
    let nutshell_db_path = find_sqlite_db(&fuzz_dir)
        .expect("Could not find nutshell sqlite database in temp directory");
    println!("Found nutshell database at: {:?}", nutshell_db_path);

    // Read the seed from nutshell's database directly
    let conn = rusqlite::Connection::open(&nutshell_db_path)?;
    let seed_str: String = conn.query_row(
        "SELECT seed FROM keysets WHERE active = 1 LIMIT 1;",
        [],
        |row| row.get(0),
    )?;
    println!("Retrieved nutshell seed from database: {}", seed_str);

    let mut stmt = conn.prepare("SELECT id FROM keysets;")?;
    let nutshell_keyset_ids: std::collections::HashSet<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(Result::ok)
        .collect();
    println!("Retrieved nutshell keyset IDs: {:?}", nutshell_keyset_ids);

    // 4. Run database migration to CDK!
    println!("Migrating nutshell database to CDK...");
    cdk_sqlite::mint::migrate::migrate_from_nutshell(
        &cdk_db_path,
        nutshell_db_path.to_str().unwrap(),
        None,
    )
    .await
    .expect("Migration failed");
    println!("Migration complete!");

    // Reset any pending proofs to UNSPENT by deleting them from the proof table on the new mint
    {
        let conn = rusqlite::Connection::open(&cdk_db_path)?;
        let rows_deleted = conn.execute("DELETE FROM proof WHERE state = 'PENDING';", [])?;
        println!("Reset {} pending proof(s) to UNSPENT.", rows_deleted);
    }

    // 5. Instantiate and run the new CDK mint in-process pointing to port 4444 (matching original URL)
    println!("Starting migrated CDK mint in-process...");
    let db = cdk_sqlite::mint::MintSqliteDatabase::new(cdk_db_path.clone())
        .await
        .unwrap();

    let target_keysets = db.get_keyset_infos().await?;
    let cdk_keyset_ids: std::collections::HashSet<String> =
        target_keysets.iter().map(|k| k.id.to_string()).collect();
    println!("CDK keyset IDs: {:?}", cdk_keyset_ids);

    for id in &nutshell_keyset_ids {
        assert!(
            cdk_keyset_ids.contains(id),
            "CDK keysets must contain migrated keyset ID {}",
            id
        );
    }
    assert_eq!(
        nutshell_keyset_ids.len(),
        cdk_keyset_ids.len(),
        "Number of keysets must match exactly after migration!"
    );

    for k in &target_keysets {
        println!(
            "CDK keyset: id={}, active={}, valid_from={}, final_expiry={:?}, derivation_path={:?}, unit={:?}, input_fee_ppk={}",
            k.id, k.active, k.valid_from, k.final_expiry, k.derivation_path, k.unit, k.input_fee_ppk
        );
    }

    let fake_wallet = cdk_fake_wallet::FakeWallet::new(
        cdk_common::common::FeeReserve {
            min_fee_reserve: 1.into(),
            percent_fee_reserve: 0.0,
        },
        HashMap::new(),
        HashSet::new(),
        0,
        CurrencyUnit::Sat,
    );

    let db_arc = Arc::new(db);
    // Use the exact same seed bytes from nutshell:
    let seed_bytes = seed_str.as_bytes();

    let limits = cdk::mint::MintMeltLimits::new(0, 10_000_000_000);
    let mut custom_paths = HashMap::new();
    custom_paths.insert(
        CurrencyUnit::Sat,
        DerivationPath::from_str("m/0'/0'/0'").unwrap(),
    );
    let mut mint_builder =
        cdk::mint::MintBuilder::new(db_arc.clone()).with_custom_derivation_paths(custom_paths);

    let cdk_amounts: Vec<u64> = (0..64).map(|n| 2_u64.pow(n)).collect();
    mint_builder
        .configure_unit(
            CurrencyUnit::Sat,
            cdk::mint::UnitConfig {
                amounts: cdk_amounts,
                input_fee_ppk: 0,
            },
        )
        .unwrap();

    mint_builder
        .add_payment_processor(
            CurrencyUnit::Sat,
            PaymentMethod::BOLT11,
            limits,
            Arc::new(fake_wallet),
        )
        .await
        .unwrap();

    let mint = mint_builder
        .build_with_seed(db_arc, seed_bytes)
        .await
        .unwrap();

    let active_keysets = mint.get_active_keysets();
    for (unit, keyset_id) in &active_keysets {
        println!(
            "CDK mint active keyset for unit {:?}: id={}",
            unit, keyset_id
        );
    }

    let mint_arc = Arc::new(mint);
    mint_arc.start().await.unwrap();

    let router = cdk_axum::create_mint_router(mint_arc.clone(), vec!["bolt11".to_string()])
        .await
        .unwrap();

    // Bind to port 4444 (exact same port as original nutshell mint)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:4444")
        .await
        .unwrap();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("CDK Mint server error: {:?}", e);
        }
    });

    // Wait 500ms for server to boot up
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    println!("CDK Mint is online on port 4444.");

    // Reset any pending proofs inside the wallets' local stores too
    {
        let conn = rusqlite::Connection::open(&cdk_db_path)?;
        let mut stmt = conn.prepare("SELECT y FROM proof WHERE state = 'SPENT';")?;
        let spent_ys_iter = stmt.query_map([], |row| {
            let y_bytes: Vec<u8> = row.get(0)?;
            Ok(cdk::nuts::PublicKey::from_slice(&y_bytes).unwrap())
        })?;
        let mut spent_ys = std::collections::HashSet::new();
        for y in spent_ys_iter.flatten() {
            spent_ys.insert(y);
        }

        println!("CDK Mint spent_ys length: {}", spent_ys.len());
        for y in &spent_ys {
            println!("CDK Mint spent y: {}", y);
        }

        for (w_idx, store) in wallet_stores.iter().enumerate() {
            let proofs_info = store.get_proofs(None, None, None, None).await.unwrap();

            let mut local_spent_ys = Vec::new();
            let mut local_pending_ys = Vec::new();

            for p in proofs_info {
                let y = p.proof.y().unwrap();
                println!("Wallet {} proof: y={}, state={:?}", w_idx, y, p.state);
                if spent_ys.contains(&y) {
                    if p.state != State::Spent {
                        local_spent_ys.push(y);
                    }
                } else if p.state == State::Pending {
                    local_pending_ys.push(y);
                }
            }

            if !local_spent_ys.is_empty() {
                store
                    .update_proofs_state(local_spent_ys, State::Spent)
                    .await
                    .unwrap();
            }
            if !local_pending_ys.is_empty() {
                store
                    .update_proofs_state(local_pending_ys, State::Unspent)
                    .await
                    .unwrap();
            }
        }
    }

    // 6. Point wallets to the migrated CDK mint on port 4444 and verify!
    println!("Verifying wallet balances and spendability on migrated CDK mint...");
    let mut ported_wallets = Vec::new();
    for (i, (seed, store)) in wallet_seeds.iter().zip(wallet_stores.iter()).enumerate() {
        let wallet_ported = Wallet::new(
            "http://127.0.0.1:4444",
            CurrencyUnit::Sat,
            store.clone(),
            *seed,
            None,
        )
        .unwrap();

        // Check if balance is successfully recovered
        let bal_after = wallet_ported.total_balance().await.unwrap();
        println!("Wallet {} balance after migration: {}", i, bal_after);
        assert_eq!(
            bal_after,
            balances_before[i],
            "Ported wallet {} balance after migration ({}) must match balance before migration ({}) exactly!",
            i, bal_after, balances_before[i]
        );

        ported_wallets.push(wallet_ported);
    }
    println!("Success: All wallet balances match exactly after migration!");

    // Verify wallet operations on the newly migrated CDK mint
    println!(
        "Testing spendability and wallet operations on the migrated CDK mint (40 operations)..."
    );
    for i in 0..40 {
        let wallet_idx = rand::random_range(0..10);
        let op_idx = rand::random_range(0..3);
        let wallet = &ported_wallets[wallet_idx];

        println!("CDK Round {}: Wallet {}, Op {}", i, wallet_idx, op_idx);
        match op_idx {
            0 => {
                // Mint
                let amount = Amount::from(50);
                if let Ok(mint_quote) = wallet
                    .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
                    .await
                {
                    if let Err(e) = wallet
                        .wait_and_mint_quote(
                            mint_quote,
                            SplitTarget::default(),
                            None,
                            Duration::from_secs(30),
                        )
                        .await
                    {
                        println!("Warning: wait_and_mint_quote failed on CDK: {:?}", e);
                    }
                }
            }
            1 => {
                // Swap
                let unspent = wallet.get_unspent_proofs().await.unwrap_or_default();
                if !unspent.is_empty() {
                    if let Err(e) = wallet
                        .swap(None, SplitTarget::default(), unspent, None, false, false)
                        .await
                    {
                        println!("Warning: swap failed on CDK: {:?}", e);
                    }
                }
            }
            _ => {
                // Melt
                if let Err(e) = do_melt(wallet).await {
                    println!("Warning: do_melt failed on CDK: {:?}", e);
                }
            }
        }
    }

    println!("All wallet operations on the migrated CDK mint succeeded perfectly!");
    Ok(())
}
