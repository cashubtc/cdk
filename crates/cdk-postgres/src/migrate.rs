use std::cell::RefCell;
use std::collections::HashSet;
use std::str::FromStr;

use bitcoin::bip32::DerivationPath;
use cdk_common::database::{
    Error, MintDatabase, MintKeysDatabase, MintProofsDatabase, MintQuotesDatabase,
    MintSignaturesDatabase,
};
use cdk_common::mint::{MeltPaymentRequest, MeltQuote, MintKeySetInfo, MintQuote, Operation};
use cdk_common::payment::PaymentIdentifier;
use cdk_common::quote_id::QuoteId;
use cdk_common::secret::Secret;
use cdk_common::{
    Amount, BlindSignature, BlindSignatureDleq, BlindedMessage, CurrencyUnit, Id, MeltQuoteState,
    MintQuoteState, PaymentMethod, Proof, PublicKey, SecretKey, State as ProofState,
};
use chrono::NaiveDateTime;

use super::{MintPgDatabase, PgConfig};

const MAX_SUPPORTED_NUTSHELL_VERSION: &str = "0.20.1";
const CHUNK_SIZE: i64 = 2000;

enum MigratedPromise {
    Signature(PublicKey, BlindSignature, Option<QuoteId>, Id),
    Message(BlindedMessage, Option<QuoteId>, Id),
}

fn parse_nutshell_version(v: &str) -> Option<(u32, u32, u32)> {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() >= 2 {
        let major = parts[0].parse::<u32>().ok()?;
        let minor = parts[1].parse::<u32>().ok()?;
        let patch = if parts.len() >= 3 {
            parts[2].parse::<u32>().ok().unwrap_or(0)
        } else {
            0
        };
        Some((major, minor, patch))
    } else {
        None
    }
}

fn parse_nutshell_timestamp(v: &str) -> u64 {
    if let Ok(ts) = v.parse::<u64>() {
        return ts;
    }
    if let Ok(ts_f) = v.parse::<f64>() {
        return ts_f as u64;
    }
    for fmt in &[
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S.%f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S.%fZ",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(v, fmt) {
            return dt.and_utc().timestamp() as u64;
        }
    }
    0
}

async fn read_keysets_postgres(
    client: &tokio_postgres::Client,
) -> Result<Vec<MintKeySetInfo>, Error> {
    let has_final_expiry: bool = client
        .query_one(
            "SELECT EXISTS (
                SELECT 1 
                FROM information_schema.columns 
                WHERE table_name='keysets' AND column_name='final_expiry'
            );",
            &[],
        )
        .await
        .map(|row| row.get(0))
        .unwrap_or(false);

    let query = if has_final_expiry {
        "SELECT id, derivation_path, valid_from::text, valid_to::text, active, version, unit, input_fee_ppk, amounts, final_expiry FROM keysets;"
    } else {
        "SELECT id, derivation_path, valid_from::text, valid_to::text, active, version, unit, input_fee_ppk, amounts, NULL::integer FROM keysets;"
    };

    let rows = client.query(query, &[])
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut keysets = Vec::new();
    for r in rows {
        let id_str: String = r.get(0);
        let derivation_path_str: String = r.get(1);
        let valid_from_str: String = r.get(2);
        let valid_to_str: Option<String> = r.get(3);
        let active: bool = r.get(4);
        let version: String = r.get(5);
        let unit_str: String = r.get(6);
        let input_fee_ppk: i32 = r.get::<_, Option<i32>>(7).unwrap_or(0);
        let amounts_str: String = r
            .get::<_, Option<String>>(8)
            .unwrap_or_else(|| "[]".to_string());
        let final_expiry_val: Option<i32> = r.get(9);

        let amounts_vec: Vec<u64> = if amounts_str.is_empty() || amounts_str == "[]" {
            (0..32).map(|i| 2_u64.pow(i)).collect()
        } else {
            serde_json::from_str(&amounts_str)
                .unwrap_or_else(|_| (0..32).map(|i| 2_u64.pow(i)).collect())
        };

        let valid_from = parse_nutshell_timestamp(&valid_from_str);
        let final_expiry = if let Some(fe) = final_expiry_val.filter(|&v| v > 0) {
            Some(fe as u64)
        } else if active {
            None
        } else {
            valid_to_str
                .as_ref()
                .map(|v| parse_nutshell_timestamp(v))
                .filter(|&ts| ts > 0)
        };

        let id = match Id::from_str(&id_str) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Skipping keyset due to invalid Keyset ID '{}': {:?}",
                    id_str,
                    e
                );
                continue;
            }
        };

        let unit = match CurrencyUnit::from_str(&unit_str) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(
                    "Skipping keyset {} due to invalid CurrencyUnit '{}': {:?}",
                    id,
                    unit_str,
                    e
                );
                continue;
            }
        };

        let derivation_path = match DerivationPath::from_str(&derivation_path_str) {
            Ok(dp) => dp,
            Err(e) => {
                tracing::warn!(
                    "Skipping keyset {} due to invalid DerivationPath '{}': {:?}",
                    id,
                    derivation_path_str,
                    e
                );
                continue;
            }
        };

        let issuer_version =
            match cdk_common::common::IssuerVersion::from_str(&format!("nutshell/{}", version)) {
                Ok(iv) => Some(iv),
                Err(e) => {
                    tracing::warn!(
                        "Skipping keyset {} due to invalid version format '{}': {:?}",
                        id,
                        version,
                        e
                    );
                    continue;
                }
            };

        keysets.push(MintKeySetInfo {
            id,
            unit,
            active,
            valid_from,
            derivation_path,
            derivation_path_index: None,
            amounts: amounts_vec,
            input_fee_ppk: input_fee_ppk as u64,
            final_expiry,
            issuer_version,
        });
    }
    Ok(keysets)
}

async fn read_mint_quotes_chunk_postgres(
    client: &tokio_postgres::Client,
    limit: i64,
    offset: i64,
) -> Result<Vec<(MintQuote, String, Option<u64>, bool, bool)>, Error> {
    let rows = client.query("SELECT quote, method, request, checking_id, unit, amount, created_time::text, paid_time::text, state, pubkey FROM mint_quotes LIMIT $1 OFFSET $2;", &[&limit, &offset])
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for r in rows {
        let quote: String = r.get(0);
        let method_str: String = r.get(1);
        let request: String = r.get(2);
        let checking_id: String = r.get(3);
        let unit_str: String = r.get(4);
        let amount: i64 = r.get(5);
        let created_time_str: Option<String> = r.get(6);
        let paid_time_str: Option<String> = r.get(7);
        let state_str: String = r.get(8);
        let pubkey_str: Option<String> = r.get(9);

        let q_id = match QuoteId::from_str(&quote) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Skipping mint quote due to invalid QuoteId '{}': {:?}",
                    quote,
                    e
                );
                continue;
            }
        };
        let unit = match CurrencyUnit::from_str(&unit_str) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(
                    "Skipping mint quote {} due to invalid CurrencyUnit '{}': {:?}",
                    quote,
                    unit_str,
                    e
                );
                continue;
            }
        };
        let created_time = created_time_str
            .as_ref()
            .map(|t| parse_nutshell_timestamp(t))
            .unwrap_or_else(cdk_common::util::unix_time);
        let expiry = created_time + 86400; // default 24h

        let request_lookup_id_kind = if checking_id.len() == 64 && hex::decode(&checking_id).is_ok()
        {
            "payment_hash"
        } else {
            "custom"
        };
        let request_lookup_id = match PaymentIdentifier::new(request_lookup_id_kind, &checking_id) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Skipping mint quote {} due to invalid PaymentIdentifier '{}': {:?}",
                    quote,
                    checking_id,
                    e
                );
                continue;
            }
        };

        let state_mapped = match state_str.to_lowercase().as_str() {
            "paid" => MintQuoteState::Paid,
            "issued" => MintQuoteState::Issued,
            _ => MintQuoteState::Unpaid,
        };

        let is_paid =
            state_mapped == MintQuoteState::Paid || state_mapped == MintQuoteState::Issued;
        let is_issued = state_mapped == MintQuoteState::Issued;

        let amount_paid = if is_paid {
            Amount::from(amount as u64).with_unit(unit.clone())
        } else {
            Amount::from(0).with_unit(unit.clone())
        };

        let amount_issued = if is_issued {
            Amount::from(amount as u64).with_unit(unit.clone())
        } else {
            Amount::from(0).with_unit(unit.clone())
        };

        let pubkey = pubkey_str
            .as_ref()
            .and_then(|pk| PublicKey::from_hex(pk).ok());

        let method = match PaymentMethod::from_str(&method_str) {
            Ok(m) => m,
            Err(_) => PaymentMethod::from("bolt11"),
        };

        let quote_obj = MintQuote::new(
            Some(q_id),
            request,
            unit.clone(),
            Some(Amount::from(amount as u64).with_unit(unit)),
            expiry,
            request_lookup_id,
            pubkey,
            amount_paid,
            amount_issued,
            method,
            created_time,
            vec![],
            vec![],
            None,
        );

        let paid_time = paid_time_str.as_ref().map(|t| parse_nutshell_timestamp(t));

        chunk.push((quote_obj, checking_id, paid_time, is_paid, is_issued));
    }
    Ok(chunk)
}

async fn read_melt_quotes_chunk_postgres(
    client: &tokio_postgres::Client,
    limit: i64,
    offset: i64,
    seen_paid_pending_lookup_ids: &RefCell<HashSet<String>>,
) -> Result<Vec<MeltQuote>, Error> {
    let rows = client.query("SELECT quote, method, request, checking_id, unit, amount, fee_reserve, paid, created_time::text, paid_time::text, state, expiry::text, proof FROM melt_quotes LIMIT $1 OFFSET $2;", &[&limit, &offset])
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for r in rows {
        let quote: String = r.get(0);
        let method_str: String = r.get(1);
        let request_str: String = r.get(2);
        let checking_id: String = r.get(3);
        let unit_str: String = r.get(4);
        let amount: i64 = r.get(5);
        let fee_reserve: i32 = r.get::<_, Option<i32>>(6).unwrap_or(0);
        let created_time_str: Option<String> = r.get(8);
        let paid_time_str: Option<String> = r.get(9);
        let state_str: String = r.get(10);
        let expiry_str: Option<String> = r.get(11);
        let payment_proof: Option<String> = r.get(12);

        let q_id = match QuoteId::from_str(&quote) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Skipping melt quote due to invalid QuoteId '{}': {:?}",
                    quote,
                    e
                );
                continue;
            }
        };
        let unit = match CurrencyUnit::from_str(&unit_str) {
            Ok(u) => u,
            Err(e) => {
                tracing::warn!(
                    "Skipping melt quote {} due to invalid CurrencyUnit '{}': {:?}",
                    quote,
                    unit_str,
                    e
                );
                continue;
            }
        };
        let created_time = created_time_str
            .as_ref()
            .map(|t| parse_nutshell_timestamp(t))
            .unwrap_or_else(cdk_common::util::unix_time);
        let expiry = expiry_str
            .as_ref()
            .map(|t| parse_nutshell_timestamp(t))
            .unwrap_or(created_time + 86400);
        let paid_time = paid_time_str.as_ref().map(|t| parse_nutshell_timestamp(t));

        let request = if let Ok(bolt11) = lightning_invoice::Bolt11Invoice::from_str(&request_str) {
            MeltPaymentRequest::Bolt11 { bolt11 }
        } else {
            serde_json::from_str(&request_str).unwrap_or_else(|_| MeltPaymentRequest::Custom {
                method: "bolt11".to_string(),
                request: request_str,
            })
        };

        let mut request_lookup_id = if checking_id.len() == 64 {
            if let Ok(bytes) = hex::decode(&checking_id) {
                if let Ok(arr) = bytes.try_into() {
                    Some(PaymentIdentifier::PaymentHash(arr))
                } else {
                    Some(PaymentIdentifier::CustomId(checking_id))
                }
            } else {
                Some(PaymentIdentifier::CustomId(checking_id))
            }
        } else {
            Some(PaymentIdentifier::CustomId(checking_id))
        };

        let state_mapped = match state_str.to_lowercase().as_str() {
            "paid" => MeltQuoteState::Paid,
            "pending" => MeltQuoteState::Pending,
            "failed" => MeltQuoteState::Failed,
            _ => MeltQuoteState::Unpaid,
        };

        if let Some(ref ref_id) = request_lookup_id {
            if state_mapped == MeltQuoteState::Paid || state_mapped == MeltQuoteState::Pending {
                let id_key = ref_id.to_string();
                let mut borrowed = seen_paid_pending_lookup_ids.borrow_mut();
                if borrowed.contains(&id_key) {
                    let dup_id = format!("{}-dup-{}", id_key, q_id);
                    request_lookup_id = Some(PaymentIdentifier::CustomId(dup_id));
                } else {
                    borrowed.insert(id_key);
                }
            }
        }

        let method = match PaymentMethod::from_str(&method_str) {
            Ok(m) => m,
            Err(_) => PaymentMethod::from("bolt11"),
        };

        let quote_res = match MeltQuote::from_db(
            q_id.clone(),
            unit,
            request,
            amount as u64,
            fee_reserve as u64,
            state_mapped,
            expiry,
            payment_proof,
            request_lookup_id,
            None,
            created_time,
            paid_time,
            method,
            None,
            None,
            vec![],
            None,
        ) {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!(
                    "Skipping melt quote {} due to serialization/mapping failure: {:?}",
                    q_id,
                    e
                );
                continue;
            }
        };
        chunk.push(quote_res);
    }
    Ok(chunk)
}

async fn read_promises_chunk_postgres(
    client: &tokio_postgres::Client,
    limit: i64,
    offset: i64,
) -> Result<Vec<MigratedPromise>, Error> {
    let rows = client.query("SELECT amount, id, b_, c_, dleq_e, dleq_s, mint_quote, melt_quote FROM promises LIMIT $1 OFFSET $2;", &[&limit, &offset])
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for r in rows {
        let amount_val: i64 = r.get(0);
        let keyset_id_str: String = r.get(1);
        let b_str: String = r.get(2);
        let c_str: Option<String> = r.get(3);
        let dleq_e_str: Option<String> = r.get(4);
        let dleq_s_str: Option<String> = r.get(5);
        let mint_quote_str: Option<String> = r.get(6);
        let melt_quote_str: Option<String> = r.get(7);

        let amount = Amount::from(amount_val as u64);
        let keyset_id = match Id::from_str(&keyset_id_str) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Skipping promise row due to invalid Keyset ID '{}': {:?}",
                    keyset_id_str,
                    e
                );
                continue;
            }
        };
        let blinded_message_pubkey = match PublicKey::from_hex(&b_str) {
            Ok(pk) => pk,
            Err(e) => {
                tracing::warn!(
                    "Skipping promise row due to invalid B_ public key '{}': {:?}",
                    b_str,
                    e
                );
                continue;
            }
        };

        let q_id = mint_quote_str
            .as_ref()
            .or(melt_quote_str.as_ref())
            .and_then(|q| QuoteId::from_str(q).ok());

        if let Some(ref c_hex) = c_str {
            let c_pk = match PublicKey::from_hex(c_hex) {
                Ok(pk) => pk,
                Err(e) => {
                    tracing::warn!(
                        "Skipping promise row due to invalid C_ public key '{}': {:?}",
                        c_hex,
                        e
                    );
                    continue;
                }
            };

            let dleq = match (dleq_e_str.as_ref(), dleq_s_str.as_ref()) {
                (Some(e), Some(s)) => {
                    let parsed_e = match SecretKey::from_hex(e) {
                        Ok(sk) => sk,
                        Err(err) => {
                            tracing::warn!(
                                "Skipping promise row due to invalid DLEQ e secret key '{}': {:?}",
                                e,
                                err
                            );
                            continue;
                        }
                    };
                    let parsed_s = match SecretKey::from_hex(s) {
                        Ok(sk) => sk,
                        Err(err) => {
                            tracing::warn!(
                                "Skipping promise row due to invalid DLEQ s secret key '{}': {:?}",
                                s,
                                err
                            );
                            continue;
                        }
                    };
                    Some(BlindSignatureDleq {
                        e: parsed_e,
                        s: parsed_s,
                    })
                }
                _ => None,
            };

            let cdk_sig = BlindSignature {
                amount,
                keyset_id,
                c: c_pk,
                dleq,
            };
            chunk.push(MigratedPromise::Signature(
                blinded_message_pubkey,
                cdk_sig,
                q_id,
                keyset_id,
            ));
        } else {
            let cdk_msg = BlindedMessage {
                amount,
                keyset_id,
                blinded_secret: blinded_message_pubkey,
                witness: None,
            };
            chunk.push(MigratedPromise::Message(cdk_msg, q_id, keyset_id));
        }
    }
    Ok(chunk)
}

async fn read_proofs_chunk_postgres(
    client: &tokio_postgres::Client,
    limit: i64,
    offset: i64,
    spent: bool,
) -> Result<Vec<(Proof, Option<QuoteId>, Id, ProofState)>, Error> {
    let query_str = if spent {
        "SELECT amount, id, c, secret, witness, melt_quote FROM proofs_used LIMIT $1 OFFSET $2;"
    } else {
        "SELECT amount, id, c, secret, NULL, melt_quote FROM proofs_pending LIMIT $1 OFFSET $2;"
    };
    let target_state = if spent {
        ProofState::Spent
    } else {
        ProofState::Pending
    };

    let rows = client
        .query(query_str, &[&limit, &offset])
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for r in rows {
        let amount_val: i64 = r.get(0);
        let id_str: String = r.get(1);
        let c_str: String = r.get(2);
        let secret_str: String = r.get(3);
        let witness_str: Option<String> = r.get(4);
        let melt_quote_str: Option<String> = r.get(5);

        let keyset_id = match Id::from_str(&id_str) {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    "Skipping proof due to invalid Keyset ID '{}': {:?}",
                    id_str,
                    e
                );
                continue;
            }
        };

        let secret = match Secret::from_str(&secret_str) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "Skipping proof due to invalid Secret '{}': {:?}",
                    secret_str,
                    e
                );
                continue;
            }
        };

        let c = match PublicKey::from_hex(&c_str) {
            Ok(pk) => pk,
            Err(e) => {
                tracing::warn!(
                    "Skipping proof due to invalid C_ public key '{}': {:?}",
                    c_str,
                    e
                );
                continue;
            }
        };

        let cdk_proof = Proof {
            amount: Amount::from(amount_val as u64),
            keyset_id,
            secret,
            c,
            witness: witness_str
                .as_ref()
                .and_then(|w| serde_json::from_str(w).ok()),
            dleq: None,
            p2pk_e: None,
        };

        let melt_q_id = melt_quote_str
            .as_ref()
            .and_then(|q| QuoteId::from_str(q).ok());

        chunk.push((cdk_proof, melt_q_id, keyset_id, target_state));
    }
    Ok(chunk)
}

/// Migrates a nutshell database to CDK Postgres database
pub async fn migrate_from_nutshell(cdk_db_url: &str, nutshell_db_url: &str) -> Result<(), Error> {
    tracing::info!("Starting nutshell database migration...");

    // Connect to source database
    let (client, connection) = tokio_postgres::connect(nutshell_db_url, tokio_postgres::NoTls)
        .await
        .map_err(|e| Error::Database(Box::new(e)))?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("Postgres connection error: {}", e);
        }
    });

    // 1. Read and validate keysets (Pre-flight checks on nutshell version)
    let nutshell_keysets = read_keysets_postgres(&client).await?;

    let max_v = parse_nutshell_version(MAX_SUPPORTED_NUTSHELL_VERSION).unwrap_or((0, 20, 1));
    for keyset in &nutshell_keysets {
        if let Some(ref version_str) = keyset.issuer_version {
            let ver_clean = version_str.to_string().replace("nutshell/", "");
            if let Some(keyset_v) = parse_nutshell_version(&ver_clean) {
                if keyset_v > max_v {
                    return Err(Error::Database(Box::new(std::io::Error::other(format!(
                        "Unsupported Nutshell version: {}. Maximum supported version is: {}.",
                        ver_clean, MAX_SUPPORTED_NUTSHELL_VERSION
                    )))));
                }
            }
        }
    }

    // 2. Setup target database connection
    let db_config = PgConfig::new(cdk_db_url, Some("disable"), Some(20), Some(10));
    let db = MintPgDatabase::new(db_config).await?;

    // 3. Pre-flight checks on target database population
    let existing_keyset_infos = db.get_keyset_infos().await?;
    if !existing_keyset_infos.is_empty() {
        return Err(Error::Database(Box::new(std::io::Error::other(
            "Target CDK database already contains keyset data! Aborting migration to prevent accidental data overwrite/corruption."
        ))));
    }

    tracing::info!("Database pre-flight checks passed.");

    // Start transactions
    let mut key_tx = MintKeysDatabase::begin_transaction(&db).await?;

    let mut skipped_keysets_count = 0;
    let mut skipped_promises_count = 0;
    let mut skipped_proofs_count = 0;
    let seen_paid_pending_lookup_ids = RefCell::new(HashSet::new());

    let mut migrated_keysets = 0;
    let mut migrated_mint_quotes = 0;
    let mut migrated_melt_quotes = 0;
    let mut _migrated_promises = 0;
    let mut migrated_promises_signed = 0;
    let mut migrated_proofs = 0;

    // Map and migrate keysets
    let mut migrated_keyset_ids = HashSet::new();
    for keyset in nutshell_keysets {
        if let Some(ref version_str) = keyset.issuer_version {
            let ver_clean = version_str.to_string().replace("nutshell/", "");
            if let Some(keyset_v) = parse_nutshell_version(&ver_clean) {
                if keyset_v < (0, 15, 0) {
                    tracing::warn!(
                        "Skipping keyset {} because it was generated under nutshell version {} (pre-0.15 keysets use a different derivation path not supported by CDK).",
                        keyset.id,
                        ver_clean
                    );
                    println!(
                        "WARNING: Skipping keyset {} because it was generated under nutshell version {} (pre-0.15 keysets use a different derivation path not supported by CDK).",
                        keyset.id,
                        ver_clean
                    );
                    skipped_keysets_count += 1;
                    continue;
                }
            }
        }

        let keyset_id = keyset.id;
        key_tx.add_keyset_info(keyset).await?;
        migrated_keyset_ids.insert(keyset_id);
        migrated_keysets += 1;
    }

    key_tx.commit().await?;
    tracing::info!("Migrated keysets successfully.");

    // Start main database transaction after keysets are committed to avoid SQLite lock deadlock
    let mut tx = MintDatabase::begin_transaction(&db).await?;

    // 4. Chunked Migration of Mint Quotes
    let mut offset = 0;
    loop {
        let chunk = read_mint_quotes_chunk_postgres(&client, CHUNK_SIZE, offset).await?;

        if chunk.is_empty() {
            break;
        }

        for (quote_obj, checking_id, paid_time_opt, is_paid, is_issued) in chunk {
            let mut acquired_quote = tx.add_mint_quote(quote_obj.clone()).await?;

            if is_paid {
                let paid_time = paid_time_opt.unwrap_or(quote_obj.created_time);
                let unit = quote_obj.unit.clone();
                let amount = quote_obj
                    .amount
                    .clone()
                    .unwrap_or_else(|| Amount::from(0).with_unit(unit));
                acquired_quote
                    .add_payment(amount, checking_id, Some(paid_time))
                    .map_err(|e| Error::Database(Box::new(std::io::Error::other(e.to_string()))))?;
            }

            if is_issued {
                let unit = quote_obj.unit.clone();
                let amount = quote_obj
                    .amount
                    .clone()
                    .unwrap_or_else(|| Amount::from(0).with_unit(unit));
                let _ = acquired_quote
                    .add_issuance(amount)
                    .map_err(|e| Error::Database(Box::new(std::io::Error::other(e.to_string()))))?;
            }

            tx.update_mint_quote(&mut acquired_quote).await?;
            migrated_mint_quotes += 1;
        }

        offset += CHUNK_SIZE;
    }
    tracing::info!("Migrated mint quotes successfully.");

    // 5. Chunked Migration of Melt Quotes
    let mut offset = 0;
    loop {
        let chunk = read_melt_quotes_chunk_postgres(
            &client,
            CHUNK_SIZE,
            offset,
            &seen_paid_pending_lookup_ids,
        )
        .await?;

        if chunk.is_empty() {
            break;
        }

        for quote in chunk {
            tx.add_melt_quote(quote).await?;
            migrated_melt_quotes += 1;
        }

        offset += CHUNK_SIZE;
    }
    tracing::info!("Migrated melt quotes successfully.");

    // 6. Chunked Migration of Promises (Blind Signatures / Blinded Messages)
    let dummy_operation = Operation::new_mint(
        Amount::ZERO,
        PaymentMethod::from_str("bolt11").unwrap_or_else(|_| PaymentMethod::from("bolt11")),
    );
    let mut offset = 0;
    loop {
        let chunk = read_promises_chunk_postgres(&client, CHUNK_SIZE, offset).await?;

        if chunk.is_empty() {
            break;
        }

        for promise in chunk {
            match promise {
                MigratedPromise::Signature(blinded_message_pubkey, cdk_sig, q_id, keyset_id) => {
                    if !migrated_keyset_ids.contains(&keyset_id) {
                        skipped_promises_count += 1;
                        continue;
                    }
                    tx.add_blind_signatures(&[blinded_message_pubkey], &[cdk_sig], q_id)
                        .await?;
                    _migrated_promises += 1;
                    migrated_promises_signed += 1;
                }
                MigratedPromise::Message(cdk_msg, q_id, keyset_id) => {
                    if !migrated_keyset_ids.contains(&keyset_id) {
                        skipped_promises_count += 1;
                        continue;
                    }
                    tx.add_blinded_messages(q_id.as_ref(), &[cdk_msg], &dummy_operation)
                        .await?;
                    _migrated_promises += 1;
                }
            }
        }

        offset += CHUNK_SIZE;
    }
    tracing::info!("Migrated promises successfully.");

    // 7. Chunked Migration of Proofs
    for spent in &[true, false] {
        let mut offset = 0;
        loop {
            let chunk = read_proofs_chunk_postgres(&client, CHUNK_SIZE, offset, *spent).await?;

            if chunk.is_empty() {
                break;
            }

            for (cdk_proof, melt_q_id, keyset_id, target_state) in chunk {
                if !migrated_keyset_ids.contains(&keyset_id) {
                    skipped_proofs_count += 1;
                    continue;
                }

                let _y = cdk_proof.y()?;
                let mut acquired = tx
                    .add_proofs(vec![cdk_proof], melt_q_id, &dummy_operation)
                    .await?;
                tx.update_proofs_state(&mut acquired, target_state).await?;
                migrated_proofs += 1;
            }

            offset += CHUNK_SIZE;
        }
    }
    tracing::info!("Migrated proofs successfully.");

    tx.commit().await?;
    tracing::info!("Transaction committed successfully.");

    // Perform verification
    let mut target_promises_signed = 0;
    let mut target_proofs = 0;
    let target_keysets = db.get_keyset_infos().await?;
    for keyset in &target_keysets {
        target_promises_signed += db.get_blind_signatures_for_keyset(&keyset.id).await?.len();
        target_proofs += db.get_proofs_by_keyset_id(&keyset.id).await?.0.len();
    }

    let target_keysets_count = target_keysets.len();
    let target_mint_quotes = db.get_mint_quotes().await?.len();
    let target_melt_quotes = db.get_melt_quotes().await?.len();

    tracing::info!("Verifying migrated data consistency...");
    if target_keysets_count != migrated_keysets {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Verification failed: Keyset count mismatch. Expected {}, found {}",
            migrated_keysets, target_keysets_count
        )))));
    }
    if target_mint_quotes != migrated_mint_quotes {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Verification failed: Mint quote count mismatch. Expected {}, found {}",
            migrated_mint_quotes, target_mint_quotes
        )))));
    }
    if target_melt_quotes != migrated_melt_quotes {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Verification failed: Melt quote count mismatch. Expected {}, found {}",
            migrated_melt_quotes, target_melt_quotes
        )))));
    }
    if target_promises_signed != migrated_promises_signed {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Verification failed: Promise signature count mismatch. Expected {}, found {}",
            migrated_promises_signed, target_promises_signed
        )))));
    }
    if target_proofs != migrated_proofs {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Verification failed: Proof count mismatch. Expected {}, found {}",
            migrated_proofs, target_proofs
        )))));
    }
    tracing::info!("Verification success: All target database row counts match migrated source records exactly!");

    if skipped_keysets_count > 0 {
        let msg = format!(
            "Migration warning: Skipped {} keyset(s), {} promise(s), and {} proof(s) because they were generated under a Nutshell version < 0.15.0.",
            skipped_keysets_count,
            skipped_promises_count,
            skipped_proofs_count
        );
        tracing::warn!("{}", msg);
        println!("\nWARNING: {}", msg);
    }

    tracing::info!(
        "Migration complete: Nutshell mint has been fully and successfully migrated to CDK!"
    );

    Ok(())
}
