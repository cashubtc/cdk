use std::collections::{BTreeMap, HashSet};
use std::path::Path;
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
use cdk_common::util::hex;
use cdk_common::{
    Amount, BlindSignature, BlindSignatureDleq, BlindedMessage, CurrencyUnit, Id, MeltQuoteState,
    MintQuoteState, PaymentMethod, Proof, PublicKey, SecretKey, State as ProofState,
};
use chrono::NaiveDateTime;
use rusqlite::OptionalExtension;

use super::MintSqliteDatabase;

const MAX_SUPPORTED_NUTSHELL_VERSION: &str = "0.20.2";
const SUPPORTED_NUTSHELL_SCHEMA_VERSION: i64 = 36;
const CHUNK_SIZE: i64 = 2000;

enum MigratedPromise {
    Signature(PublicKey, BlindSignature, Option<QuoteId>, Id, u64),
    Message(BlindedMessage, Option<QuoteId>, Id, u64),
}

type PendingMeltRequest = (QuoteId, Amount<CurrencyUnit>, Amount<CurrencyUnit>);

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

fn val_to_string(val: rusqlite::types::Value) -> String {
    match val {
        rusqlite::types::Value::Null => "".to_string(),
        rusqlite::types::Value::Integer(i) => i.to_string(),
        rusqlite::types::Value::Real(f) => f.to_string(),
        rusqlite::types::Value::Text(s) => s,
        rusqlite::types::Value::Blob(b) => String::from_utf8_lossy(&b).to_string(),
    }
}

fn source_count(conn: &rusqlite::Connection, table: &str) -> Result<usize, Error> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
        .map(|count| count as usize)
        .map_err(|e| Error::Database(Box::new(e)))
}

fn source_proof_count(conn: &rusqlite::Connection) -> Result<usize, Error> {
    conn.query_row(
        "SELECT COUNT(*) FROM (SELECT y FROM proofs_used UNION SELECT y FROM proofs_pending)",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count as usize)
    .map_err(|e| Error::Database(Box::new(e)))
}

fn validate_nutshell_schema(conn: &rusqlite::Connection) -> Result<(), Error> {
    let version = conn
        .query_row(
            "SELECT version FROM dbversions WHERE db = 'mint'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| Error::Database(Box::new(e)))?;
    if version != SUPPORTED_NUTSHELL_SCHEMA_VERSION {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Unsupported Nutshell mint schema version {version}; expected version {SUPPORTED_NUTSHELL_SCHEMA_VERSION} from Nutshell {MAX_SUPPORTED_NUTSHELL_VERSION}"
        )))));
    }
    Ok(())
}

fn validate_melt_quote_lookup_ids(conn: &rusqlite::Connection) -> Result<(), Error> {
    let duplicate = conn
        .query_row(
            "SELECT checking_id FROM melt_quotes WHERE lower(state) IN ('paid', 'pending') GROUP BY checking_id HAVING COUNT(*) > 1 LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| Error::Database(Box::new(e)))?;
    if let Some(checking_id) = duplicate {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Nutshell contains multiple paid or pending melt quotes for checking_id {checking_id}"
        )))));
    }
    Ok(())
}

fn query_pairs(conn: &rusqlite::Connection, sql: &str) -> Result<Vec<(String, i64)>, Error> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| Error::Database(Box::new(e)))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| Error::Database(Box::new(e)))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| Error::Database(Box::new(e)))
}

fn liability_totals(
    conn: &rusqlite::Connection,
    sql: &str,
) -> Result<BTreeMap<String, (i64, i64)>, Error> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| Error::Database(Box::new(e)))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
        .map_err(|e| Error::Database(Box::new(e)))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| Error::Database(Box::new(e)))
}

/// Independently verify an already migrated Nutshell 0.20.2 SQLite database.
pub fn verify_nutshell_migration(
    cdk_db_path: &Path,
    nutshell_db_path: &str,
    db_password: Option<&str>,
) -> Result<(), Error> {
    let source = rusqlite::Connection::open_with_flags(
        nutshell_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| Error::Database(Box::new(e)))?;
    let target = rusqlite::Connection::open_with_flags(
        cdk_db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|e| Error::Database(Box::new(e)))?;
    if let Some(password) = db_password {
        target
            .pragma_update(None, "key", password)
            .map_err(|e| Error::Database(Box::new(e)))?;
    }
    validate_nutshell_schema(&source)?;

    for (source_table, target_table) in [
        ("keysets", "keyset"),
        ("mint_quotes", "mint_quote"),
        ("melt_quotes", "melt_quote"),
        ("promises", "blind_signature"),
    ] {
        let expected = source_count(&source, source_table)?;
        let actual = source_count(&target, target_table)?;
        if expected != actual {
            return Err(Error::Database(Box::new(std::io::Error::other(format!(
                "Verification failed for {source_table}: source has {expected} rows, target has {actual}"
            )))));
        }
    }
    let expected_proofs = source_proof_count(&source)?;
    let actual_proofs = source_count(&target, "proof")?;
    if expected_proofs != actual_proofs {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Verification failed for proofs: source has {expected_proofs} rows, target has {actual_proofs}"
        )))));
    }

    let source_quote_accounting = query_pairs(
        &source,
        "SELECT quote || ':' || COALESCE(amount_paid, 0), COALESCE(amount_issued, 0) FROM mint_quotes ORDER BY quote",
    )?;
    let target_quote_accounting = query_pairs(
        &target,
        "SELECT id || ':' || amount_paid, amount_issued FROM mint_quote ORDER BY id",
    )?;
    if source_quote_accounting != target_quote_accounting {
        return Err(Error::Database(Box::new(std::io::Error::other(
            "Verification failed: mint quote accounting differs",
        ))));
    }

    let source_order = query_pairs(
        &source,
        "SELECT lower(b_), COALESCE(order_index, 0) FROM promises ORDER BY lower(b_)",
    )?;
    let target_order = query_pairs(
        &target,
        "SELECT lower(hex(blinded_message)), order_index FROM blind_signature ORDER BY lower(hex(blinded_message))",
    )?;
    if source_order != target_order {
        return Err(Error::Database(Box::new(std::io::Error::other(
            "Verification failed: promise order indexes differ",
        ))));
    }

    let source_liabilities = liability_totals(
        &source,
        "SELECT k.id, COALESCE((SELECT SUM(amount) FROM promises p WHERE p.id = k.id AND p.c_ IS NOT NULL), 0), COALESCE((SELECT SUM(amount) FROM proofs_used u WHERE u.id = k.id), 0) FROM keysets k ORDER BY k.id",
    )?;
    let target_liabilities = liability_totals(
        &target,
        "SELECT keyset_id, total_issued, total_redeemed FROM keyset_amounts ORDER BY keyset_id",
    )?;
    if source_liabilities != target_liabilities {
        return Err(Error::Database(Box::new(std::io::Error::other(
            "Verification failed: per-keyset liabilities differ",
        ))));
    }

    tracing::info!(
        keysets = source_count(&source, "keysets")?,
        mint_quotes = source_count(&source, "mint_quotes")?,
        melt_quotes = source_count(&source, "melt_quotes")?,
        promises = source_count(&source, "promises")?,
        proofs = expected_proofs,
        "Independent Nutshell migration verification succeeded"
    );
    Ok(())
}

fn read_keysets_sqlite(conn: &rusqlite::Connection) -> Result<Vec<MintKeySetInfo>, Error> {
    let mut has_final_expiry = false;
    if let Ok(mut stmt) = conn.prepare("PRAGMA table_info(keysets);") {
        let mut rows = stmt.query([]).map_err(|e| Error::Database(Box::new(e)))?;
        while let Some(row) = rows.next().map_err(|e| Error::Database(Box::new(e)))? {
            let name: String = row.get(1).map_err(|e| Error::Database(Box::new(e)))?;
            if name == "final_expiry" {
                has_final_expiry = true;
                break;
            }
        }
    }

    let query = if has_final_expiry {
        "SELECT id, derivation_path, valid_from, valid_to, active, version, unit, input_fee_ppk, amounts, final_expiry FROM keysets;"
    } else {
        "SELECT id, derivation_path, valid_from, valid_to, active, version, unit, input_fee_ppk, amounts, NULL FROM keysets;"
    };

    let mut stmt = conn
        .prepare(query)
        .map_err(|e| Error::Database(Box::new(e)))?;
    let keysets_iter = stmt
        .query_map([], |row| {
            let id_str: String = row.get(0)?;
            let derivation_path_str: String = row.get(1)?;
            let valid_from_val = row.get::<_, rusqlite::types::Value>(2)?;
            let _valid_to_val = row.get::<_, Option<rusqlite::types::Value>>(3)?;
            let active: bool = row.get(4)?;
            let version: String = row.get(5)?;
            let unit_str: String = row.get(6)?;
            let input_fee_ppk: i64 = row.get(7)?;
            let amounts_str: String = row.get(8)?;
            let final_expiry_val: Option<i64> = row.get(9)?;

            let valid_from_str = val_to_string(valid_from_val);

            let amounts_vec: Vec<u64> = if amounts_str.is_empty() || amounts_str == "[]" {
                (0..64).map(|i| 2_u64.pow(i)).collect()
            } else {
                serde_json::from_str(&amounts_str)
                    .unwrap_or_else(|_| (0..64).map(|i| 2_u64.pow(i)).collect())
            };

            let valid_from = parse_nutshell_timestamp(&valid_from_str);
            let final_expiry = final_expiry_val.filter(|&v| v > 0).map(|v| v as u64);

            let id = match Id::from_str(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(
                        "Skipping keyset due to invalid Keyset ID '{}': {:?}",
                        id_str,
                        e
                    );
                    return Ok(None);
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
                    return Ok(None);
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
                    return Ok(None);
                }
            };

            let issuer_version =
                match cdk_common::common::IssuerVersion::from_str(&format!("nutshell/{}", version))
                {
                    Ok(iv) => Some(iv),
                    Err(e) => {
                        tracing::warn!(
                            "Skipping keyset {} due to invalid version format '{}': {:?}",
                            id,
                            version,
                            e
                        );
                        return Ok(None);
                    }
                };

            Ok(Some(MintKeySetInfo {
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
            }))
        })
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut keysets = Vec::new();
    for k in keysets_iter {
        match k {
            Ok(Some(ks)) => keysets.push(ks),
            Ok(None) => {} // Skipped
            Err(e) => {
                tracing::warn!("Failed to retrieve keyset row from SQLite: {:?}", e);
            }
        }
    }
    Ok(keysets)
}

type MigratedMintQuoteInfo = (
    MintQuote,
    String,
    Option<u64>,
    Amount<CurrencyUnit>,
    Amount<CurrencyUnit>,
);

fn read_mint_quotes_chunk_sqlite(
    conn: &rusqlite::Connection,
    limit: i64,
    offset: i64,
) -> Result<Vec<MigratedMintQuoteInfo>, Error> {
    let mut stmt = conn.prepare("SELECT quote, method, request, checking_id, unit, amount, created_time, paid_time, state, pubkey, amount_paid, amount_issued, updated_at FROM mint_quotes ORDER BY quote LIMIT ? OFFSET ?;")
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mint_quotes_iter = stmt
        .query_map([limit, offset], |row| {
            let quote: String = row.get(0)?;
            let method_str: String = row.get(1)?;
            let request: String = row.get(2)?;
            let checking_id: String = row.get(3)?;
            let unit_str: String = row.get(4)?;
            let amount: i64 = row.get(5)?;
            let created_time_val = row.get::<_, Option<rusqlite::types::Value>>(6)?;
            let paid_time_val = row.get::<_, Option<rusqlite::types::Value>>(7)?;
            let state_str: String = row.get(8)?;
            let pubkey_str: Option<String> = row.get(9)?;
            let stored_amount_paid: Option<i64> = row.get(10)?;
            let stored_amount_issued: Option<i64> = row.get(11)?;
            let updated_at_val = row.get::<_, Option<rusqlite::types::Value>>(12)?;

            let created_time_str = created_time_val.map(val_to_string);
            let paid_time_str = paid_time_val.map(val_to_string);
            let updated_at = updated_at_val
                .map(val_to_string)
                .map(|value| parse_nutshell_timestamp(&value));

            let q_id = match QuoteId::from_str(&quote) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(
                        "Skipping mint quote due to invalid QuoteId '{}': {:?}",
                        quote,
                        e
                    );
                    return Ok(None);
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
                    return Ok(None);
                }
            };
            let created_time = created_time_str
                .as_ref()
                .map(|t| parse_nutshell_timestamp(t))
                .unwrap_or_else(cdk_common::util::unix_time);
            let expiry = created_time + 86400; // default 24h

            let request_lookup_id_kind =
                if checking_id.len() == 64 && hex::decode(&checking_id).is_ok() {
                    "payment_hash"
                } else {
                    "custom"
                };
            let request_lookup_id =
                match PaymentIdentifier::new(request_lookup_id_kind, &checking_id) {
                    Ok(id) => id,
                    Err(e) => {
                        tracing::warn!(
                            "Skipping mint quote {} due to invalid PaymentIdentifier '{}': {:?}",
                            quote,
                            checking_id,
                            e
                        );
                        return Ok(None);
                    }
                };

            let state_mapped = match state_str.to_lowercase().as_str() {
                "paid" | "pending" => MintQuoteState::Paid,
                "issued" => MintQuoteState::Issued,
                _ => MintQuoteState::Unpaid,
            };

            let amount_paid = Amount::from(stored_amount_paid.unwrap_or_else(|| {
                if state_mapped == MintQuoteState::Paid || state_mapped == MintQuoteState::Issued {
                    amount
                } else {
                    0
                }
            }) as u64)
            .with_unit(unit.clone());
            let amount_issued = Amount::from(stored_amount_issued.unwrap_or_else(|| {
                if state_mapped == MintQuoteState::Issued {
                    amount
                } else {
                    0
                }
            }) as u64)
            .with_unit(unit.clone());
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
                Some(Amount::from(amount as u64).with_unit(unit.clone())),
                expiry,
                request_lookup_id,
                pubkey,
                Amount::ZERO.with_unit(unit.clone()),
                Amount::ZERO.with_unit(unit.clone()),
                method,
                created_time,
                updated_at.unwrap_or(created_time),
                vec![],
                vec![],
                None,
            );

            let paid_time = paid_time_str.as_ref().map(|t| parse_nutshell_timestamp(t));

            Ok(Some((
                quote_obj,
                checking_id,
                paid_time,
                amount_paid,
                amount_issued,
            )))
        })
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for q in mint_quotes_iter {
        match q {
            Ok(Some(quote_info)) => chunk.push(quote_info),
            Ok(None) => {} // Skipped
            Err(e) => {
                tracing::warn!("Failed to retrieve mint quote row from SQLite: {:?}", e);
            }
        }
    }
    Ok(chunk)
}

fn read_melt_quotes_chunk_sqlite(
    conn: &rusqlite::Connection,
    limit: i64,
    offset: i64,
) -> Result<Vec<MeltQuote>, Error> {
    let mut stmt = conn.prepare("SELECT quote, method, request, checking_id, unit, amount, fee_reserve, paid, created_time, paid_time, state, expiry, proof FROM melt_quotes ORDER BY quote LIMIT ? OFFSET ?;")
        .map_err(|e| Error::Database(Box::new(e)))?;
    let melt_quotes_iter = stmt
        .query_map([limit, offset], |row| {
            let quote: String = row.get(0)?;
            let method_str: String = row.get(1)?;
            let request_str: String = row.get(2)?;
            let checking_id: String = row.get(3)?;
            let unit_str: String = row.get(4)?;
            let amount: i64 = row.get(5)?;
            let fee_reserve: i64 = row.get::<_, Option<i64>>(6)?.unwrap_or(0);
            let created_time_val = row.get::<_, Option<rusqlite::types::Value>>(8)?;
            let paid_time_val = row.get::<_, Option<rusqlite::types::Value>>(9)?;
            let state_str: String = row.get(10)?;
            let expiry_val = row.get::<_, Option<rusqlite::types::Value>>(11)?;
            let payment_proof: Option<String> = row.get(12)?;

            let created_time_str = created_time_val.map(val_to_string);
            let paid_time_str = paid_time_val.map(val_to_string);
            let expiry_str = expiry_val.map(val_to_string);

            let q_id = match QuoteId::from_str(&quote) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(
                        "Skipping melt quote due to invalid QuoteId '{}': {:?}",
                        quote,
                        e
                    );
                    return Ok(None);
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
                    return Ok(None);
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

            let request = if let Ok(bolt11) =
                lightning_invoice::Bolt11Invoice::from_str(&request_str)
            {
                MeltPaymentRequest::Bolt11 { bolt11 }
            } else {
                serde_json::from_str(&request_str).unwrap_or_else(|_| MeltPaymentRequest::Custom {
                    method: "bolt11".to_string(),
                    request: request_str,
                })
            };

            let request_lookup_id = if checking_id.len() == 64 {
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
                // CDK's persisted melt quote schema has no FAILED state. A
                // failed Nutshell payment is not paid or in flight, so preserve
                // it as UNPAID rather than aborting the entire migration.
                "failed" => MeltQuoteState::Unpaid,
                _ => MeltQuoteState::Unpaid,
            };

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
                    return Ok(None);
                }
            };
            Ok(Some(quote_res))
        })
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for q in melt_quotes_iter {
        match q {
            Ok(Some(quote)) => chunk.push(quote),
            Ok(None) => {} // Skipped
            Err(e) => {
                tracing::warn!("Failed to retrieve melt quote row from SQLite: {:?}", e);
            }
        }
    }
    Ok(chunk)
}

fn read_promises_chunk_sqlite(
    conn: &rusqlite::Connection,
    limit: i64,
    offset: i64,
) -> Result<Vec<MigratedPromise>, Error> {
    let mut stmt = conn.prepare("SELECT amount, id, b_, c_, dleq_e, dleq_s, mint_quote, melt_quote, order_index FROM promises ORDER BY b_ LIMIT ? OFFSET ?;")
        .map_err(|e| Error::Database(Box::new(e)))?;
    let promises_iter = stmt
        .query_map([limit, offset], |row| {
            let amount_val: i64 = row.get(0)?;
            let keyset_id_str: String = row.get(1)?;
            let b_str: String = row.get(2)?;
            let c_str: Option<String> = row.get(3)?;
            let dleq_e_str: Option<String> = row.get(4)?;
            let dleq_s_str: Option<String> = row.get(5)?;
            let mint_quote_str: Option<String> = row.get(6)?;
            let melt_quote_str: Option<String> = row.get(7)?;
            let order_index = row.get::<_, Option<i64>>(8)?.unwrap_or(0) as u64;

            let amount = Amount::from(amount_val as u64);
            let keyset_id = match Id::from_str(&keyset_id_str) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("Skipping promise row due to invalid Keyset ID '{}': {:?}", keyset_id_str, e);
                    return Ok(None);
                }
            };
            let blinded_message_pubkey = match PublicKey::from_hex(&b_str) {
                Ok(pk) => pk,
                Err(e) => {
                    tracing::warn!("Skipping promise row due to invalid B_ public key '{}': {:?}", b_str, e);
                    return Ok(None);
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
                        tracing::warn!("Skipping promise row due to invalid C_ public key '{}': {:?}", c_hex, e);
                        return Ok(None);
                    }
                };

                let dleq = match (dleq_e_str.as_ref(), dleq_s_str.as_ref()) {
                    (Some(e), Some(s)) => {
                        let parsed_e = match SecretKey::from_hex(e) {
                            Ok(sk) => sk,
                            Err(err) => {
                                tracing::warn!("Skipping promise row due to invalid DLEQ e secret key '{}': {:?}", e, err);
                                return Ok(None);
                            }
                        };
                        let parsed_s = match SecretKey::from_hex(s) {
                            Ok(sk) => sk,
                            Err(err) => {
                                tracing::warn!("Skipping promise row due to invalid DLEQ s secret key '{}': {:?}", s, err);
                                return Ok(None);
                            }
                        };
                        Some(BlindSignatureDleq { e: parsed_e, s: parsed_s })
                    }
                    _ => None,
                };

                let cdk_sig = BlindSignature {
                    amount,
                    keyset_id,
                    c: c_pk,
                    dleq,
                };
                Ok(Some(MigratedPromise::Signature(
                    blinded_message_pubkey,
                    cdk_sig,
                    q_id,
                    keyset_id,
                    order_index,
                )))
            } else {
                let cdk_msg = BlindedMessage {
                    amount,
                    keyset_id,
                    blinded_secret: blinded_message_pubkey,
                    witness: None,
                };
                Ok(Some(MigratedPromise::Message(
                    cdk_msg,
                    q_id,
                    keyset_id,
                    order_index,
                )))
            }
        })
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for p in promises_iter {
        match p {
            Ok(Some(promise)) => chunk.push(promise),
            Ok(None) => {} // Skipped
            Err(e) => {
                tracing::warn!("Failed to retrieve promise row from SQLite: {:?}", e);
            }
        }
    }
    Ok(chunk)
}

type MigratedProofInfo = (Proof, Option<QuoteId>, Id, ProofState);

fn read_proofs_chunk_sqlite(
    conn: &rusqlite::Connection,
    limit: i64,
    offset: i64,
    spent: bool,
) -> Result<Vec<MigratedProofInfo>, Error> {
    let query_str = if spent {
        "SELECT amount, id, c, secret, witness, melt_quote FROM proofs_used ORDER BY secret LIMIT ? OFFSET ?;"
    } else {
        "SELECT amount, id, c, secret, witness, melt_quote FROM proofs_pending ORDER BY secret LIMIT ? OFFSET ?;"
    };
    let target_state = if spent {
        ProofState::Spent
    } else {
        ProofState::Pending
    };

    let mut stmt = conn
        .prepare(query_str)
        .map_err(|e| Error::Database(Box::new(e)))?;
    let proofs_iter = stmt
        .query_map([limit, offset], |row| {
            let amount_val: i64 = row.get(0)?;
            let id_str: String = row.get(1)?;
            let c_str: String = row.get(2)?;
            let secret_str: String = row.get(3)?;
            let witness_str: Option<String> = row.get(4)?;
            let melt_quote_str: Option<String> = row.get(5)?;

            let keyset_id = match Id::from_str(&id_str) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(
                        "Skipping proof due to invalid Keyset ID '{}': {:?}",
                        id_str,
                        e
                    );
                    return Ok(None);
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
                    return Ok(None);
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
                    return Ok(None);
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

            Ok(Some((cdk_proof, melt_q_id, keyset_id, target_state)))
        })
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut chunk = Vec::new();
    for p in proofs_iter {
        match p {
            Ok(Some(proof_info)) => chunk.push(proof_info),
            Ok(None) => {} // Skipped
            Err(e) => {
                tracing::warn!("Failed to retrieve proof row from SQLite: {:?}", e);
            }
        }
    }
    Ok(chunk)
}

fn read_pending_melt_requests_sqlite(
    conn: &rusqlite::Connection,
) -> Result<Vec<PendingMeltRequest>, Error> {
    let mut stmt = conn
        .prepare(
            "SELECT p.melt_quote, SUM(p.amount), COALESCE(SUM(k.input_fee_ppk), 0), m.unit
             FROM proofs_pending p
             JOIN keysets k ON k.id = p.id
             JOIN melt_quotes m ON m.quote = p.melt_quote
             WHERE p.melt_quote IS NOT NULL
             GROUP BY p.melt_quote, m.unit",
        )
        .map_err(|e| Error::Database(Box::new(e)))?;
    let rows = stmt
        .query_map([], |row| {
            let quote: String = row.get(0)?;
            let inputs_amount: i64 = row.get(1)?;
            let fee_ppk: i64 = row.get(2)?;
            let unit: String = row.get(3)?;
            Ok((quote, inputs_amount, fee_ppk, unit))
        })
        .map_err(|e| Error::Database(Box::new(e)))?;
    let mut requests = Vec::new();
    for row in rows {
        let (quote, inputs_amount, fee_ppk, unit) =
            row.map_err(|e| Error::Database(Box::new(e)))?;
        let quote_id = QuoteId::from_str(&quote)
            .map_err(|e| Error::Database(Box::new(std::io::Error::other(e.to_string()))))?;
        let unit = CurrencyUnit::from_str(&unit)
            .map_err(|e| Error::Database(Box::new(std::io::Error::other(e.to_string()))))?;
        requests.push((
            quote_id,
            Amount::from(inputs_amount as u64).with_unit(unit.clone()),
            Amount::from((fee_ppk as u64).div_ceil(1000)).with_unit(unit),
        ));
    }
    Ok(requests)
}

async fn migrate_from_nutshell_into(
    cdk_db_path: &Path,
    nutshell_db_path: &str,
    db_password: Option<String>,
) -> Result<(), Error> {
    tracing::info!("Starting nutshell database migration...");
    let verification_password = db_password.clone();

    // Connect to source database
    let sqlite_conn =
        rusqlite::Connection::open(nutshell_db_path).map_err(|e| Error::Database(Box::new(e)))?;
    validate_nutshell_schema(&sqlite_conn)?;
    validate_melt_quote_lookup_ids(&sqlite_conn)?;

    let source_keysets = source_count(&sqlite_conn, "keysets")?;
    let source_mint_quotes = source_count(&sqlite_conn, "mint_quotes")?;
    let source_melt_quotes = source_count(&sqlite_conn, "melt_quotes")?;
    let source_promises = source_count(&sqlite_conn, "promises")?;
    let source_spent_proofs = source_count(&sqlite_conn, "proofs_used")?;
    let source_pending_proofs = source_count(&sqlite_conn, "proofs_pending")?;
    // A failed Nutshell melt can leave the same proof in both state tables.
    // CDK stores one row per Y, with the stricter state winning.
    let source_proofs = source_proof_count(&sqlite_conn)?;

    // 1. Read and validate keysets (Pre-flight checks on nutshell version)
    let nutshell_keysets = read_keysets_sqlite(&sqlite_conn)?;
    if nutshell_keysets.len() != source_keysets {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Source validation failed: read {} of {source_keysets} keysets",
            nutshell_keysets.len()
        )))));
    }

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
                if keyset_v < (0, 15, 0) {
                    return Err(Error::Database(Box::new(std::io::Error::other(format!(
                        "Unsupported Nutshell keyset {} from version {ver_clean}; pre-0.15 keysets cannot be migrated or verified by CDK",
                        keyset.id
                    )))));
                }
            }
        }
    }

    // 2. Setup target database connection
    let db = match db_password {
        Some(pass) => MintSqliteDatabase::new((cdk_db_path.to_path_buf(), pass)).await?,
        None => MintSqliteDatabase::new(cdk_db_path.to_path_buf()).await?,
    };

    // 3. Pre-flight checks on target database population
    let existing_keyset_infos = db.get_keyset_infos().await?;
    if !existing_keyset_infos.is_empty()
        || !db.get_mint_quotes().await?.is_empty()
        || !db.get_melt_quotes().await?.is_empty()
        || !db.get_total_issued().await?.is_empty()
        || !db.get_total_redeemed().await?.is_empty()
    {
        return Err(Error::Database(Box::new(std::io::Error::other(
            "Target CDK database already contains mint data! Aborting migration to prevent accidental data overwrite/corruption."
        ))));
    }

    tracing::info!("Database pre-flight checks passed.");

    // Start transactions
    let mut key_tx = MintKeysDatabase::begin_transaction(&db).await?;

    let mut skipped_keysets_count = 0;
    let mut skipped_promises_count = 0;
    let mut skipped_proofs_count = 0;

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
        key_tx.add_keyset_info(keyset).await.inspect_err(|_| {
            tracing::error!("Failed migrating Keyset: {:?}", keyset_id);
            println!("Failed migrating Keyset: {}", keyset_id);
        })?;
        migrated_keyset_ids.insert(keyset_id);
        migrated_keysets += 1;
    }

    key_tx.commit().await?;
    tracing::info!("Migrated keysets successfully.");

    // Start main database transaction after keysets are committed to avoid SQLite lock deadlock
    let mut tx = MintDatabase::begin_transaction(&db).await?;

    // 4. Chunked Migration of Mint Quotes
    let mut offset = 0;
    while offset < source_mint_quotes as i64 {
        let chunk = read_mint_quotes_chunk_sqlite(&sqlite_conn, CHUNK_SIZE, offset)?;

        for (quote_obj, checking_id, paid_time_opt, amount_paid, amount_issued) in chunk {
            let mut acquired_quote =
                tx.add_mint_quote(quote_obj.clone())
                    .await
                    .inspect_err(|_| {
                        tracing::error!("Failed migrating Mint Quote: {:?}", quote_obj.id);
                        println!("Failed migrating Mint Quote: {}", quote_obj.id);
                    })?;

            if amount_paid.value() > 0 {
                let paid_time = paid_time_opt.unwrap_or(quote_obj.created_time);
                acquired_quote
                    .add_payment(amount_paid, checking_id, Some(paid_time))
                    .map_err(|e| Error::Database(Box::new(std::io::Error::other(e.to_string()))))?;
            }

            if amount_issued.value() > 0 {
                let _ = acquired_quote
                    .add_issuance(amount_issued)
                    .map_err(|e| Error::Database(Box::new(std::io::Error::other(e.to_string()))))?;
            }

            tx.update_mint_quote(&mut acquired_quote)
                .await
                .inspect_err(|_| {
                    tracing::error!("Failed updating Mint Quote: {:?}", acquired_quote.id);
                    println!("Failed updating Mint Quote: {}", acquired_quote.id);
                })?;
            migrated_mint_quotes += 1;
        }

        offset += CHUNK_SIZE;
    }
    tracing::info!("Migrated mint quotes successfully.");

    // 5. Chunked Migration of Melt Quotes
    let mut offset = 0;
    while offset < source_melt_quotes as i64 {
        let chunk = read_melt_quotes_chunk_sqlite(&sqlite_conn, CHUNK_SIZE, offset)?;

        for quote in chunk {
            let quote_id = quote.id.clone();
            tx.add_melt_quote(quote).await.inspect_err(|_| {
                tracing::error!("Failed migrating Melt Quote: {:?}", quote_id);
                println!("Failed migrating Melt Quote: {}", quote_id);
            })?;
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
    while offset < source_promises as i64 {
        let chunk = read_promises_chunk_sqlite(&sqlite_conn, CHUNK_SIZE, offset)?;

        for promise in chunk {
            match promise {
                MigratedPromise::Signature(
                    blinded_message_pubkey,
                    cdk_sig,
                    q_id,
                    keyset_id,
                    order_index,
                ) => {
                    if !migrated_keyset_ids.contains(&keyset_id) {
                        skipped_promises_count += 1;
                        continue;
                    }
                    tx.add_blind_signatures_with_order(
                        &[blinded_message_pubkey],
                        std::slice::from_ref(&cdk_sig),
                        q_id,
                        &[order_index],
                    )
                    .await
                    .inspect_err(|_| {
                        tracing::error!(
                            "Failed migrating Promise Signature: msg={:?}, keyset={:?}, c={:?}",
                            blinded_message_pubkey,
                            keyset_id,
                            cdk_sig.c
                        );
                        println!(
                            "Failed migrating Promise Signature: msg={}, keyset={}, c={}",
                            blinded_message_pubkey, keyset_id, cdk_sig.c
                        );
                    })?;
                    _migrated_promises += 1;
                    migrated_promises_signed += 1;
                }
                MigratedPromise::Message(cdk_msg, q_id, keyset_id, order_index) => {
                    if !migrated_keyset_ids.contains(&keyset_id) {
                        skipped_promises_count += 1;
                        continue;
                    }
                    tx.add_blinded_messages_with_order(
                        q_id.as_ref(),
                        std::slice::from_ref(&cdk_msg),
                        &dummy_operation,
                        &[order_index],
                    )
                    .await
                    .inspect_err(|_| {
                        tracing::error!(
                            "Failed migrating Promise Message: msg={:?}, keyset={:?}",
                            cdk_msg.blinded_secret,
                            keyset_id
                        );
                        println!(
                            "Failed migrating Promise Message: msg={}, keyset={}",
                            cdk_msg.blinded_secret, keyset_id
                        );
                    })?;
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
        let table_count = if *spent {
            source_spent_proofs
        } else {
            source_pending_proofs
        };
        while offset < table_count as i64 {
            let chunk = read_proofs_chunk_sqlite(&sqlite_conn, CHUNK_SIZE, offset, *spent)?;

            for (cdk_proof, melt_q_id, keyset_id, target_state) in chunk {
                if !migrated_keyset_ids.contains(&keyset_id) {
                    skipped_proofs_count += 1;
                    continue;
                }

                let _y = cdk_proof.y()?;
                let mut acquired = tx
                    .add_proofs(vec![cdk_proof.clone()], melt_q_id, &dummy_operation)
                    .await
                    .inspect_err(|_| {
                        tracing::error!(
                            "Failed migrating Proof (adding): secret={:?}, keyset={:?}",
                            cdk_proof.secret,
                            keyset_id
                        );
                        println!(
                            "Failed migrating Proof (adding): secret={}, keyset={}",
                            cdk_proof.secret, keyset_id
                        );
                    })?;
                tx.update_proofs_state(&mut acquired, target_state)
                    .await
                    .inspect_err(|_| {
                        tracing::error!(
                            "Failed migrating Proof (updating state): secret={:?}, keyset={:?}",
                            cdk_proof.secret,
                            keyset_id
                        );
                        println!(
                            "Failed migrating Proof (updating state): secret={}, keyset={}",
                            cdk_proof.secret, keyset_id
                        );
                    })?;
                migrated_proofs += 1;
            }

            offset += CHUNK_SIZE;
        }
    }
    tracing::info!("Migrated proofs successfully.");

    for (quote_id, inputs_amount, inputs_fee) in read_pending_melt_requests_sqlite(&sqlite_conn)? {
        tx.add_melt_request(&quote_id, inputs_amount, inputs_fee)
            .await?;
    }

    if source_keysets != migrated_keysets + skipped_keysets_count
        || source_mint_quotes != migrated_mint_quotes
        || source_melt_quotes != migrated_melt_quotes
        || source_promises != _migrated_promises + skipped_promises_count
        || source_proofs != migrated_proofs + skipped_proofs_count
    {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Source verification failed: source/migrated counts differ (keysets {source_keysets}/{migrated_keysets}, mint quotes {source_mint_quotes}/{migrated_mint_quotes}, melt quotes {source_melt_quotes}/{migrated_melt_quotes}, promises {source_promises}/{_migrated_promises}, proofs {source_proofs}/{migrated_proofs})"
        )))));
    }

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

    verify_nutshell_migration(
        cdk_db_path,
        nutshell_db_path,
        verification_password.as_deref(),
    )?;

    Ok(())
}

/// Migrates a Nutshell database into a new CDK SQLite database atomically.
pub async fn migrate_from_nutshell(
    cdk_db_path: &Path,
    nutshell_db_path: &str,
    db_password: Option<String>,
) -> Result<(), Error> {
    if cdk_db_path.exists() {
        return Err(Error::Database(Box::new(std::io::Error::other(format!(
            "Target CDK database {} already exists; migration requires a new target path",
            cdk_db_path.display()
        )))));
    }
    let file_name = cdk_db_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("cdk-mintd.sqlite");
    let staging_path = cdk_db_path.with_file_name(format!(".{file_name}.migration-staging"));
    if staging_path.exists() {
        std::fs::remove_file(&staging_path).map_err(|e| Error::Database(Box::new(e)))?;
    }

    match migrate_from_nutshell_into(&staging_path, nutshell_db_path, db_password).await {
        Ok(()) => {
            std::fs::rename(&staging_path, cdk_db_path).map_err(|e| Error::Database(Box::new(e)))
        }
        Err(error) => {
            if let Err(cleanup_error) = std::fs::remove_file(&staging_path) {
                if cleanup_error.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(%cleanup_error, "Could not remove failed migration staging database");
                }
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_nutshell_0202_schema_version() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory database");
        conn.execute(
            "CREATE TABLE dbversions (db TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            [],
        )
        .expect("create dbversions");
        conn.execute(
            "INSERT INTO dbversions (db, version) VALUES ('mint', 36)",
            [],
        )
        .expect("insert supported version");

        validate_nutshell_schema(&conn).expect("0.20.2 schema should be accepted");
    }

    #[test]
    fn rejects_other_nutshell_schema_versions() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory database");
        conn.execute(
            "CREATE TABLE dbversions (db TEXT PRIMARY KEY, version INTEGER NOT NULL)",
            [],
        )
        .expect("create dbversions");
        conn.execute(
            "INSERT INTO dbversions (db, version) VALUES ('mint', 35)",
            [],
        )
        .expect("insert unsupported version");

        let error = validate_nutshell_schema(&conn).expect_err("old schema should be rejected");
        assert!(error.to_string().contains("schema version 35"));
    }

    #[test]
    fn empty_filtered_page_does_not_mean_end_of_source() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("in-memory database");
        conn.execute_batch(
            "CREATE TABLE promises (
                amount INTEGER NOT NULL,
                id TEXT NOT NULL,
                b_ TEXT NOT NULL UNIQUE,
                c_ TEXT,
                dleq_e TEXT,
                dleq_s TEXT,
                mint_quote TEXT,
                melt_quote TEXT,
                order_index INTEGER
            );",
        )
        .expect("create promises");
        let tx = conn.transaction().expect("begin source fixture");
        for index in 0..CHUNK_SIZE {
            tx.execute(
                "INSERT INTO promises (amount, id, b_, order_index) VALUES (1, '0000000000000000', ?1, 0)",
                [format!("00-invalid-{index:04}")],
            )
            .expect("insert malformed promise");
        }
        tx.execute(
            "INSERT INTO promises (amount, id, b_, order_index) VALUES (1, '0000000000000000', '0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798', 7)",
            [],
        )
        .expect("insert valid promise after malformed page");
        tx.commit().expect("commit source fixture");

        assert_eq!(source_count(&conn, "promises").expect("source count"), 2001);
        assert!(read_promises_chunk_sqlite(&conn, CHUNK_SIZE, 0)
            .expect("read malformed page")
            .is_empty());
        let final_page =
            read_promises_chunk_sqlite(&conn, CHUNK_SIZE, CHUNK_SIZE).expect("read final page");
        assert_eq!(final_page.len(), 1, "later valid rows must still be read");
    }

    #[tokio::test]
    async fn failed_migration_does_not_leave_partial_target() {
        let id = uuid::Uuid::new_v4();
        let source_path = std::env::temp_dir().join(format!("nutshell-invalid-{id}.sqlite"));
        let target_path = std::env::temp_dir().join(format!("cdk-partial-{id}.sqlite"));
        let source = rusqlite::Connection::open(&source_path).expect("source fixture");
        source
            .execute_batch(
                "CREATE TABLE dbversions (db TEXT PRIMARY KEY, version INTEGER NOT NULL);
                 INSERT INTO dbversions (db, version) VALUES ('mint', 36);",
            )
            .expect("create incomplete source");
        drop(source);

        migrate_from_nutshell(
            &target_path,
            source_path.to_str().expect("utf8 source path"),
            None,
        )
        .await
        .expect_err("incomplete source must fail");
        assert!(
            !target_path.exists(),
            "failed migration left a target database"
        );

        let _ = std::fs::remove_file(source_path);
    }

    #[tokio::test]
    async fn existing_target_is_rejected_without_modification() {
        let id = uuid::Uuid::new_v4();
        let target_path = std::env::temp_dir().join(format!("cdk-existing-{id}.sqlite"));
        let target = rusqlite::Connection::open(&target_path).expect("target fixture");
        target
            .execute("CREATE TABLE sentinel (value TEXT NOT NULL)", [])
            .expect("create sentinel");
        target
            .execute("INSERT INTO sentinel VALUES ('keep')", [])
            .expect("insert sentinel");
        drop(target);

        migrate_from_nutshell(&target_path, "/does/not/matter.sqlite", None)
            .await
            .expect_err("existing target must be rejected");
        let target = rusqlite::Connection::open(&target_path).expect("reopen target");
        let sentinel: String = target
            .query_row("SELECT value FROM sentinel", [], |row| row.get(0))
            .expect("sentinel remains");
        assert_eq!(sentinel, "keep");
        drop(target);
        let _ = std::fs::remove_file(target_path);
    }
}
