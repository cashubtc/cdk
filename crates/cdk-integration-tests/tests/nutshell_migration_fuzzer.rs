use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bip39::Mnemonic;
use bitcoin::bip32::DerivationPath;
use bitcoin::hashes::Hash;
use cdk::amount::{Amount, SplitTarget};
use cdk::cdk_database::{MintDatabase, MintKeysDatabase, WalletDatabase};
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{CurrencyUnit, MeltQuoteState, PaymentMethod, ProofsMethods, State};
use cdk::wallet::Wallet;
use cdk_common::wallet::KeysetLoadPolicy;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const DEFAULT_FUZZ_SEED: u64 = 0x20_02_cd_c0_de;

#[derive(Debug, Default)]
struct Coverage {
    mints: usize,
    swaps: usize,
    melts: usize,
    rotations: usize,
}

impl Coverage {
    fn assert_complete(&self) {
        assert!(self.mints > 0, "fuzzer did not complete a mint");
        assert!(self.swaps > 0, "fuzzer did not complete a swap");
        assert!(self.melts > 0, "fuzzer did not complete a melt");
        assert!(self.rotations > 0, "fuzzer did not rotate a keyset");
    }
}

fn fuzz_seed() -> Result<u64> {
    match std::env::var("CDK_MIGRATION_FUZZ_SEED") {
        Ok(seed) => seed
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid CDK_MIGRATION_FUZZ_SEED '{seed}': {e}")),
        Err(_) => Ok(DEFAULT_FUZZ_SEED),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Liability {
    issued: i64,
    redeemed: i64,
}

fn source_liabilities(conn: &rusqlite::Connection) -> Result<BTreeMap<String, Liability>> {
    let mut result = BTreeMap::new();
    let mut stmt = conn.prepare("SELECT id FROM keysets ORDER BY id")?;
    let ids: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for id in ids {
        let issued = conn.query_row(
            "SELECT COALESCE(SUM(amount), 0) FROM promises WHERE id = ?1 AND c_ IS NOT NULL",
            [&id],
            |row| row.get(0),
        )?;
        let redeemed = conn.query_row(
            "SELECT COALESCE(SUM(amount), 0) FROM proofs_used WHERE id = ?1",
            [&id],
            |row| row.get(0),
        )?;
        result.insert(id, Liability { issued, redeemed });
    }
    Ok(result)
}

fn target_liabilities(conn: &rusqlite::Connection) -> Result<BTreeMap<String, Liability>> {
    let mut stmt = conn.prepare(
        "SELECT keyset_id, total_issued, total_redeemed FROM keyset_amounts ORDER BY keyset_id",
    )?;
    let liabilities = stmt
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                Liability {
                    issued: row.get(1)?,
                    redeemed: row.get(2)?,
                },
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok(liabilities)
}

fn query_strings(conn: &rusqlite::Connection, sql: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let values = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()
        .map_err(anyhow::Error::from)?;
    Ok(values)
}

fn verify_semantic_manifest(
    source: &rusqlite::Connection,
    target: &rusqlite::Connection,
) -> Result<()> {
    let source_melts = query_strings(
        source,
        "SELECT quote || '|' || method || '|' || request || '|' || unit || '|' || amount || '|' || COALESCE(fee_reserve, 0) || '|' || CASE upper(state) WHEN 'FAILED' THEN 'UNPAID' ELSE upper(state) END || '|' || COALESCE(proof, '') FROM melt_quotes ORDER BY quote",
    )?;
    let target_melts = query_strings(
        target,
        "SELECT id || '|' || payment_method || '|' || COALESCE(json_extract(request, '$.Bolt11.bolt11'), json_extract(request, '$.Custom.request')) || '|' || unit || '|' || amount || '|' || fee_reserve || '|' || state || '|' || COALESCE(payment_proof, '') FROM melt_quote ORDER BY id",
    )?;
    assert_eq!(source_melts, target_melts, "melt quote manifest mismatch");

    let source_promises = query_strings(
        source,
        "SELECT lower(b_) || '|' || amount || '|' || id || '|' || lower(COALESCE(c_, '')) || '|' || lower(COALESCE(dleq_e, '')) || '|' || lower(COALESCE(dleq_s, '')) || '|' || COALESCE(mint_quote, melt_quote, '') || '|' || COALESCE(order_index, 0) FROM promises ORDER BY lower(b_)",
    )?;
    let target_promises = query_strings(
        target,
        "SELECT lower(hex(blinded_message)) || '|' || amount || '|' || keyset_id || '|' || lower(hex(COALESCE(c, x''))) || '|' || lower(COALESCE(dleq_e, '')) || '|' || lower(COALESCE(dleq_s, '')) || '|' || COALESCE(quote_id, '') || '|' || order_index FROM blind_signature ORDER BY lower(hex(blinded_message))",
    )?;
    assert_eq!(
        source_promises, target_promises,
        "promise manifest mismatch"
    );

    let source_proofs = query_strings(
        source,
        "SELECT lower(y) || '|' || amount || '|' || id || '|' || secret || '|' || lower(c) || '|' || COALESCE(witness, '') || '|SPENT|' || COALESCE(melt_quote, '') FROM proofs_used UNION ALL SELECT lower(y) || '|' || amount || '|' || id || '|' || secret || '|' || lower(c) || '|' || COALESCE(witness, '') || '|PENDING|' || COALESCE(melt_quote, '') FROM proofs_pending p WHERE NOT EXISTS (SELECT 1 FROM proofs_used u WHERE u.y = p.y) ORDER BY 1",
    )?;
    let target_proofs = query_strings(
        target,
        "SELECT lower(hex(y)) || '|' || amount || '|' || keyset_id || '|' || secret || '|' || lower(hex(c)) || '|' || COALESCE(witness, '') || '|' || state || '|' || COALESCE(quote_id, '') FROM proof ORDER BY 1",
    )?;
    assert_eq!(source_proofs, target_proofs, "proof manifest mismatch");

    let source_liabilities = source_liabilities(source)?;
    let target_liabilities = target_liabilities(target)?;
    assert_eq!(
        source_liabilities, target_liabilities,
        "keyset liabilities mismatch"
    );
    for (keyset, liability) in source_liabilities {
        assert!(
            liability.issued >= liability.redeemed,
            "negative outstanding liability for keyset {keyset}"
        );
    }
    Ok(())
}

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

async fn do_swap(wallet: &Wallet) -> Result<bool> {
    let unspent = wallet.get_unspent_proofs().await?;
    if unspent.is_empty() {
        return Ok(false);
    }

    // Swap/split the proofs
    wallet
        .swap(None, SplitTarget::default(), unspent, None, false, false)
        .await?;
    println!("Successfully performed swap");
    Ok(true)
}

async fn do_melt(wallet: &Wallet) -> Result<bool> {
    let balance = wallet.total_balance().await?;
    if balance < Amount::from(20) {
        return Ok(false);
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
    match prepared.confirm().await {
        Ok(response) if response.state() == MeltQuoteState::Paid => {
            println!("Successfully performed melt");
            Ok(true)
        }
        Ok(response) => {
            println!(
                "FakeWallet melt ended in state {}; recording no coverage",
                response.state()
            );
            Ok(false)
        }
        Err(error) => {
            // FakeWallet intentionally injects payment failures. They remain in
            // the source state but do not count as successful melt coverage.
            println!("FakeWallet melt failed ({error}); recording no coverage");
            Ok(false)
        }
    }
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
    let seed = fuzz_seed()?;
    let mut rng = StdRng::seed_from_u64(seed);
    let mut operation_log = Vec::new();
    let mut coverage = Coverage::default();
    println!("Migration fuzz seed: {seed} (replay with CDK_MIGRATION_FUZZ_SEED={seed})");

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
            println!("Sourcing nutshell mint from Docker container (cashubtc/nutshell:0.20.2)...");
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
                    "cashubtc/nutshell:0.20.2",
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
    for _ in 0..240 {
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
        panic!("Nutshell mint failed to start on port 4444 within 120 seconds.");
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
        coverage.mints += 1;
        operation_log.push(format!("initial mint wallet={i}"));
    }

    // Mandatory deterministic coverage. Random operations below extend this state but
    // are not allowed to be the only way a migration-critical state is reached.
    assert!(do_swap(&wallets[0]).await?, "required swap was skipped");
    coverage.swaps += 1;
    operation_log.push("required swap wallet=0".to_string());
    let mut required_melt_completed = false;
    for wallet in wallets.iter().skip(1).take(5) {
        if do_melt(wallet).await? {
            required_melt_completed = true;
            break;
        }
    }
    assert!(required_melt_completed, "required melt was skipped");
    coverage.melts += 1;
    operation_log.push("required successful melt".to_string());
    rotate_nutshell_keyset(&container_name, &poetry_path).await?;
    coverage.rotations += 1;
    operation_log.push("required keyset rotation".to_string());
    for wallet in &wallets {
        wallet.keysets(KeysetLoadPolicy::Refresh).await?;
    }
    // Use a fresh wallet after rotation. Existing wallet instances intentionally
    // retain mint metadata, which would make this a cache test instead of a
    // migration fixture and can select the now-inactive keyset.
    let post_rotation_store = Arc::new(cdk_sqlite::wallet::memory::empty().await?);
    let post_rotation_mnemonic = Mnemonic::generate(12)?;
    let post_rotation_seed = post_rotation_mnemonic.to_seed_normalized("");
    let post_rotation_wallet = Wallet::new(
        "http://127.0.0.1:4444",
        CurrencyUnit::Sat,
        post_rotation_store.clone(),
        post_rotation_seed,
        None,
    )?;
    post_rotation_wallet
        .keysets(KeysetLoadPolicy::Refresh)
        .await?;
    do_mint(&post_rotation_wallet, &container_name, &poetry_path).await?;
    wallets.push(post_rotation_wallet);
    wallet_seeds.push(post_rotation_seed);
    wallet_stores.push(post_rotation_store);
    coverage.mints += 1;
    operation_log.push("required post-rotation mint wallet=10".to_string());

    // 2. Perform random operations
    println!("Performing random wallet operations (12 operations)...");
    for i in 0..12 {
        // Successful and failed melt states are created deterministically above
        // and below. Reusing a wallet after Nutshell's fault-injecting FakeWallet
        // fails a melt can leave its local proof view stale and make later random
        // swaps test the harness instead of migration.
        let op_idx = rng.random_range(0..2);
        let wallet_idx = match op_idx {
            // Only the fresh wallet has post-rotation mint metadata. The other
            // wallets remain useful for swap and melt coverage.
            0 => wallets.len() - 1,
            // Wallets 1-5 are reserved for the mandatory fault-injecting melt
            // attempts and may have a stale local proof view after a failure.
            _ => rng.random_range(6..wallets.len()),
        };
        let wallet = &wallets[wallet_idx];

        println!("Round {}: Wallet {}, Op {}", i, wallet_idx, op_idx);
        match op_idx {
            0 => {
                do_mint(wallet, &container_name, &poetry_path).await?;
                coverage.mints += 1;
                operation_log.push(format!("round={i} mint wallet={wallet_idx}"));
            }
            1 => {
                if do_swap(wallet).await? {
                    coverage.swaps += 1;
                    operation_log.push(format!("round={i} swap wallet={wallet_idx}"));
                }
            }
            _ => unreachable!("random operation index is bounded to mint and swap"),
        }
    }
    coverage.assert_complete();
    println!("Pre-migration coverage: {coverage:?}");
    println!("Replay log:\n{}", operation_log.join("\n"));

    // Create a deterministic interrupted-operation state. Nutshell normally writes
    // this row between accepting proofs and completing a melt; constructing it after
    // the mint is stopped avoids racing the server while guaranteeing coverage.
    let forced_pending_proof = wallets[0]
        .get_unspent_proofs()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("wallet 0 has no proof for pending-state coverage"))?;
    let forced_pending_y = forced_pending_proof.y()?;
    wallet_stores[0]
        .update_proofs_state(vec![forced_pending_y], State::Pending)
        .await?;

    // 3. Stop nutshell mint process
    println!("Stopping nutshell mint python process...");
    cleanup_guard.stop_nutshell();
    println!("Nutshell mint stopped successfully.");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Fix permissions of nutshell database directory using Docker if running under Linux/UNIX
    #[cfg(unix)]
    {
        println!("Fixing permissions of nutshell database directory using Docker...");
        let uid_out = std::process::Command::new("id").arg("-u").output();
        let gid_out = std::process::Command::new("id").arg("-g").output();
        if let (Ok(uid_res), Ok(gid_res)) = (uid_out, gid_out) {
            let uid = String::from_utf8_lossy(&uid_res.stdout).trim().to_string();
            let gid = String::from_utf8_lossy(&gid_res.stdout).trim().to_string();
            let _ = std::process::Command::new("docker")
                .args([
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/data", fuzz_dir.to_str().unwrap()),
                    "cashubtc/nutshell:0.20.2",
                    "chown",
                    "-R",
                    &format!("{}:{}", uid, gid),
                    "/data",
                ])
                .output();
        }
    }

    // Find nutshell sqlite database
    let nutshell_db_path = find_sqlite_db(&fuzz_dir)
        .expect("Could not find nutshell sqlite database in temp directory");
    println!("Found nutshell database at: {:?}", nutshell_db_path);

    // Read the seed from nutshell's database directly
    let conn = rusqlite::Connection::open(&nutshell_db_path)?;

    // Fault-injected melts can leave the test wallet's local proof state stale
    // even though Nutshell committed the proof as spent. Reconcile against the
    // source of truth before capturing the pre-migration balance baseline.
    let mut stmt = conn.prepare("SELECT y FROM proofs_used")?;
    let spent_ys: std::collections::HashSet<cdk::nuts::PublicKey> = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(Result::ok)
        .filter_map(|y| cdk::nuts::PublicKey::from_hex(&y).ok())
        .collect();
    for store in &wallet_stores {
        let local_spent_ys = store
            .get_proofs(None, None, None, None)
            .await?
            .into_iter()
            .filter_map(|proof_info| {
                let y = proof_info.proof.y().ok()?;
                (spent_ys.contains(&y) && proof_info.state != State::Spent).then_some(y)
            })
            .collect::<Vec<_>>();
        if !local_spent_ys.is_empty() {
            store
                .update_proofs_state(local_spent_ys, State::Spent)
                .await?;
        }
    }

    let mut balances_before = Vec::new();
    for (i, wallet) in wallets.iter().enumerate() {
        let balance = wallet.total_balance().await?;
        println!("Wallet {i} balance before migration: {balance}");
        balances_before.push(balance);
    }

    conn.execute(
        "INSERT INTO proofs_pending (amount, id, c, secret, y, witness, created, melt_quote) VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch(), NULL)",
        rusqlite::params![
            u64::from(forced_pending_proof.amount) as i64,
            forced_pending_proof.keyset_id.to_string(),
            forced_pending_proof.c.to_hex(),
            forced_pending_proof.secret.to_string(),
            forced_pending_y.to_hex(),
            forced_pending_proof
                .witness
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
        ],
    )?;
    let failed_melt_quote = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO melt_quotes (quote, method, request, checking_id, unit, amount, fee_reserve, paid, created_time, paid_time, fee_paid, proof, state, expiry) VALUES (?1, 'bolt11', 'failed-fixture', ?2, 'sat', 2, 0, 0, unixepoch(), NULL, 0, NULL, 'FAILED', unixepoch() + 3600)",
        rusqlite::params![failed_melt_quote, format!("checking-{failed_melt_quote}")],
    )?;
    println!("Inserted deterministic pending proof {forced_pending_y}");
    let seed_str: String = conn.query_row(
        "SELECT seed FROM keysets WHERE active = 1 LIMIT 1;",
        [],
        |row| row.get(0),
    )?;
    println!("Retrieved nutshell seed from database: {}", seed_str);
    let active_keyset_id: String = conn.query_row(
        "SELECT id FROM keysets WHERE active = 1 LIMIT 1",
        [],
        |row| row.get(0),
    )?;

    // Deterministic 0.20.2 accounting fixtures cover states that ordinary Bolt11
    // FakeWallet traffic cannot reliably produce: partial issuance and overpayment.
    for (state, amount_paid, amount_issued) in [
        ("UNPAID", 0_i64, 0_i64),
        ("PAID", 60, 20),
        ("PAID", 130, 100),
        ("ISSUED", 100, 100),
    ] {
        let quote = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO mint_quotes (quote, method, request, checking_id, unit, amount, created_time, paid_time, issued_time, state, pubkey, amount_paid, amount_issued, updated_at) VALUES (?1, 'bolt11', ?2, ?3, 'sat', 100, unixepoch(), unixepoch(), unixepoch(), ?4, NULL, ?5, ?6, unixepoch())",
            rusqlite::params![quote, format!("fixture-{quote}"), format!("checking-{quote}"), state, amount_paid, amount_issued],
        )?;
    }

    let pending_melt_quote = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO melt_quotes (quote, method, request, checking_id, unit, amount, fee_reserve, paid, created_time, paid_time, fee_paid, proof, state, expiry) VALUES (?1, 'bolt11', 'pending-fixture', ?2, 'sat', 2, 0, 0, unixepoch(), NULL, 0, NULL, 'PENDING', unixepoch() + 3600)",
        rusqlite::params![pending_melt_quote, format!("checking-{pending_melt_quote}")],
    )?;
    conn.execute(
        "UPDATE proofs_pending SET melt_quote = ?1 WHERE y = ?2",
        rusqlite::params![pending_melt_quote, forced_pending_y.to_hex()],
    )?;
    for (order_index, blinded_message) in [
        (
            1_i64,
            "0379be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        ),
        (
            0_i64,
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        ),
    ] {
        conn.execute(
            "INSERT INTO promises (amount, id, b_, c_, created, melt_quote, order_index) VALUES (1, ?1, ?2, NULL, unixepoch(), ?3, ?4)",
            rusqlite::params![active_keyset_id, blinded_message, pending_melt_quote, order_index],
        )?;
    }

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

    // Nutshell 0.20.2 persists cumulative quote accounting and promise ordering.
    // Both are required for safe quote recovery after the cutover.
    {
        let source = rusqlite::Connection::open(&nutshell_db_path)?;
        let target = rusqlite::Connection::open(&cdk_db_path)?;

        let mut source_quotes = source.prepare(
            "SELECT quote, COALESCE(amount_paid, 0), COALESCE(amount_issued, 0) FROM mint_quotes ORDER BY quote",
        )?;
        let source_quotes: Vec<(String, i64, i64)> = source_quotes
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<_, _>>()?;
        let mut target_quotes =
            target.prepare("SELECT id, amount_paid, amount_issued FROM mint_quote ORDER BY id")?;
        let target_quotes: Vec<(String, i64, i64)> = target_quotes
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            source_quotes, target_quotes,
            "mint quote accounting must be preserved"
        );

        let mut source_order = source.prepare(
            "SELECT lower(b_), COALESCE(order_index, 0) FROM promises ORDER BY lower(b_)",
        )?;
        let source_order: Vec<(String, i64)> = source_order
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        let mut target_order = target.prepare(
            "SELECT lower(hex(blinded_message)), order_index FROM blind_signature ORDER BY lower(hex(blinded_message))",
        )?;
        let target_order: Vec<(String, i64)> = target_order
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            source_order, target_order,
            "promise order indexes must be preserved"
        );
        verify_semantic_manifest(&source, &target)?;
        let pending_state: String = target.query_row(
            "SELECT state FROM proof WHERE y = ?1",
            [forced_pending_y.to_bytes().to_vec()],
            |row| row.get(0),
        )?;
        assert_eq!(
            pending_state, "PENDING",
            "pending proof state must survive migration"
        );
    }

    // 5. Instantiate and run the new CDK mint in-process pointing to port 4444 (matching original URL)
    println!("Starting migrated CDK mint in-process...");
    let db = cdk_sqlite::mint::MintSqliteDatabase::new(cdk_db_path.clone())
        .await
        .unwrap();

    let mut recovery_tx = MintDatabase::begin_transaction(&db).await?;
    let recovered_melt = recovery_tx
        .get_melt_request_and_blinded_messages(&cdk_common::quote_id::QuoteId::from_str(
            &pending_melt_quote,
        )?)
        .await?
        .expect("pending melt recovery metadata must be migrated");
    assert_eq!(recovered_melt.change_outputs.len(), 2);
    assert_eq!(
        recovered_melt.change_outputs[0].blinded_secret.to_hex(),
        "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
    );
    assert_eq!(
        recovered_melt.change_outputs[1].blinded_secret.to_hex(),
        "0379be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
    );
    recovery_tx.commit().await?;

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

    // Reconcile only spent wallet proofs. Pending proofs must remain pending: deleting
    // or rewriting them here would mask a migration failure in the exact state that is
    // most important for interrupted-operation recovery.
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

            for p in proofs_info {
                let y = p.proof.y().unwrap();
                println!("Wallet {} proof: y={}, state={:?}", w_idx, y, p.state);
                if spent_ys.contains(&y) && p.state != State::Spent {
                    local_spent_ys.push(y);
                }
            }

            if !local_spent_ys.is_empty() {
                store
                    .update_proofs_state(local_spent_ys, State::Spent)
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

    // Force every outstanding wallet proof through CDK's cryptographic verification.
    // A row-count or balance-only check cannot detect a wrong seed/derivation path.
    for (i, wallet) in ported_wallets.iter().enumerate() {
        let proofs = wallet.get_unspent_proofs().await?;
        if !proofs.is_empty() {
            wallet
                .swap(None, SplitTarget::default(), proofs, None, false, false)
                .await?;
            println!("Cryptographically revalidated wallet {i} proofs through a swap");
        }
    }

    // Verify wallet operations on the newly migrated CDK mint
    println!(
        "Testing spendability and wallet operations on the migrated CDK mint (40 operations)..."
    );
    for i in 0..40 {
        let wallet_idx = rng.random_range(0..ported_wallets.len());
        let op_idx = rng.random_range(0..3);
        let wallet = &ported_wallets[wallet_idx];

        println!("CDK Round {}: Wallet {}, Op {}", i, wallet_idx, op_idx);
        match op_idx {
            0 => {
                // Mint
                let amount = Amount::from(50);
                let mint_quote = wallet
                    .mint_quote(PaymentMethod::BOLT11, Some(amount), None, None)
                    .await?;
                wallet
                    .wait_and_mint_quote(
                        mint_quote,
                        SplitTarget::default(),
                        None,
                        Duration::from_secs(30),
                    )
                    .await?;
            }
            1 => {
                // Swap
                let unspent = wallet.get_unspent_proofs().await?;
                if !unspent.is_empty() {
                    wallet
                        .swap(None, SplitTarget::default(), unspent, None, false, false)
                        .await?;
                }
            }
            _ => {
                // Melt
                do_melt(wallet).await?;
            }
        }
    }

    println!("All wallet operations on the migrated CDK mint succeeded perfectly!");
    Ok(())
}
