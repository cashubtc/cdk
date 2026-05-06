//! Keyset amounts helper — bulk reads current values and performs math in Rust

use std::collections::HashMap;
use std::str::FromStr;

use cdk_common::database::Error;
use cdk_common::Id;

use crate::database::DatabaseExecutor;
use crate::stmt::query;
use crate::{column_as_number, column_as_string, unpack_into};

pub(crate) struct KeysetAmounts {
    pub total_issued: u64,
    pub total_redeemed: u64,
    pub fee_collected: u64,
}

/// Fetches all `keyset_amounts` rows for the given keyset IDs in a single query,
/// locking them with `FOR UPDATE`. If any keyset ID has no row yet, inserts a
/// default (all zeros) row and retries, so the caller is guaranteed a locked
/// entry for every requested ID.
async fn get_keyset_amounts_bulk<C>(
    conn: &C,
    keyset_ids: &[Id],
) -> Result<HashMap<Id, KeysetAmounts>, Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    if keyset_ids.is_empty() {
        return Ok(HashMap::new());
    }

    loop {
        let rows = query(
            r#"SELECT keyset_id, total_issued, total_redeemed, fee_collected
           FROM keyset_amounts
           WHERE keyset_id IN (:keyset_ids)
           FOR UPDATE"#,
        )?
        .bind_vec(
            "keyset_ids",
            keyset_ids.iter().map(|id| id.to_string()).collect(),
        )?
        .fetch_all(conn)
        .await?;

        let results = rows
            .into_iter()
            .map(|row| {
                unpack_into!(let (keyset_id, total_issued, total_redeemed, fee_collected) = row);
                Ok::<_, Error>((
                    column_as_string!(keyset_id, Id::from_str, Id::from_bytes),
                    KeysetAmounts {
                        total_issued: column_as_number!(total_issued),
                        total_redeemed: column_as_number!(total_redeemed),
                        fee_collected: column_as_number!(fee_collected),
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;

        if results.len() == keyset_ids.len() {
            break Ok(results);
        }

        // Insert default rows for missing keysets so the retry locks them
        for id in keyset_ids {
            if !results.contains_key(id) {
                query(
                    r#"
                    INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, fee_collected)
                    VALUES (:keyset_id, 0, 0, 0)
                    ON CONFLICT (keyset_id) DO NOTHING
                    "#,
                )?
                .bind("keyset_id", id.to_string())
                .execute(conn)
                .await?;
            }
        }
    }
}

pub(crate) async fn increment<C>(
    conn: &C,
    deltas: HashMap<Id, u64>,
    column: &str,
    get_field: fn(&KeysetAmounts) -> u64,
    set_field: fn(&mut KeysetAmounts, u64),
) -> Result<(), Error>
where
    C: DatabaseExecutor + Send + Sync,
{
    if deltas.is_empty() {
        return Ok(());
    }

    let keyset_ids: Vec<Id> = deltas.keys().copied().collect();
    let mut existing = get_keyset_amounts_bulk(conn, &keyset_ids).await?;

    for (keyset_id, delta) in deltas {
        let mut amounts = if let Some(amounts) = existing.remove(&keyset_id) {
            amounts
        } else {
            // Unlikely to happen since get_keyset_amounts_bulk will do the upsert
            continue;
        };
        let new_value = get_field(&amounts)
            .checked_add(delta)
            .ok_or(Error::AmountOverflow)?;
        set_field(&mut amounts, new_value);

        query(&format!(
            r#"
                INSERT INTO keyset_amounts (keyset_id, total_issued, total_redeemed, fee_collected)
                VALUES (:keyset_id, :total_issued, :total_redeemed, :fee_collected)
                ON CONFLICT (keyset_id)
                DO UPDATE SET {column} = :{column}
                "#
        ))?
        .bind("keyset_id", keyset_id.to_string())
        .bind("total_issued", amounts.total_issued)
        .bind("total_redeemed", amounts.total_redeemed)
        .bind("fee_collected", amounts.fee_collected)
        .execute(conn)
        .await?;
    }

    Ok(())
}
