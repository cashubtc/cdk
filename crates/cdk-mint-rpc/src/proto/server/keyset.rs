//! Keyset administration service.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use cdk::mint::MintKeySetInfo;
use cdk::nuts::{CurrencyUnit, Id};
use cdk::Amount;
use tonic::{Request, Response, Status};

use super::MintRPCServer;
use crate::keyset::keyset_service_server::KeysetService;

impl MintRPCServer {
    /// Rotates to the next keyset for the given unit
    async fn rotate_keyset(
        &self,
        unit: CurrencyUnit,
        amounts: Vec<u64>,
        input_fee_ppk: Option<u64>,
        use_keyset_v2: Option<bool>,
        final_expiry: Option<u64>,
    ) -> Result<MintKeySetInfo, Status> {
        self.ensure_mutation_allowed().await?;
        self.mint
            .rotate_keyset(
                unit,
                amounts,
                input_fee_ppk.unwrap_or(0),
                use_keyset_v2.unwrap_or(true),
                final_expiry,
            )
            .await
            .map_err(|_| Status::invalid_argument("Could not rotate keyset".to_string()))
    }
}

#[tonic::async_trait]
impl KeysetService for MintRPCServer {
    /// Rotates to the next keyset for the specified currency unit
    async fn rotate_next_keyset(
        &self,
        request: Request<crate::keyset::RotateNextKeysetRequest>,
    ) -> Result<Response<crate::keyset::RotateNextKeysetResponse>, Status> {
        let request = request.into_inner();

        let unit = CurrencyUnit::from_str(&request.unit)
            .map_err(|_| Status::invalid_argument("Invalid unit".to_string()))?;

        let keyset_info = self
            .rotate_keyset(
                unit,
                request.amounts,
                request.input_fee_ppk,
                request.use_keyset_v2,
                request.final_expiry,
            )
            .await?;

        Ok(Response::new(crate::keyset::RotateNextKeysetResponse {
            id: keyset_info.id.to_string(),
            unit: keyset_info.unit.to_string(),
            amounts: keyset_info.amounts,
            input_fee_ppk: keyset_info.input_fee_ppk,
        }))
    }

    /// Retrieves ecash issued and redeemed totals by unit and keyset
    async fn get_keyset_totals(
        &self,
        request: Request<crate::keyset::GetKeysetTotalsRequest>,
    ) -> Result<Response<crate::keyset::GetKeysetTotalsResponse>, Status> {
        let request = request.into_inner();

        let unit_filter = match request.unit {
            Some(ref u) if !u.trim().is_empty() => Some(
                CurrencyUnit::from_str(u.trim())
                    .map_err(|_| Status::invalid_argument("Invalid unit"))?,
            ),
            Some(_) => return Err(Status::invalid_argument("unit cannot be empty")),
            None => None,
        };

        let keyset_id_filter = match request.keyset_id {
            Some(ref kid) if !kid.trim().is_empty() => Some(
                Id::from_str(kid.trim())
                    .map_err(|_| Status::invalid_argument("Invalid keyset_id"))?,
            ),
            Some(_) => return Err(Status::invalid_argument("keyset_id cannot be empty")),
            None => None,
        };

        let total_issued = self
            .mint
            .total_issued()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;
        let total_redeemed = self
            .mint
            .total_redeemed()
            .await
            .map_err(|err| Status::internal(err.to_string()))?;

        // Collect all known keyset IDs and their currency units from mint keysets,
        // excluding Auth keysets which are not ecash tokens.
        let mut keyset_units: HashMap<Id, CurrencyUnit> = self
            .mint
            .keysets()
            .keysets
            .into_iter()
            .filter(|k| k.unit != CurrencyUnit::Auth)
            .map(|k| (k.id, k.unit))
            .collect();

        // Ensure any additional keysets from issued or redeemed are discovered
        for kid in total_issued.keys().chain(total_redeemed.keys()) {
            if !keyset_units.contains_key(kid) {
                if let Some(info) = self.mint.get_keyset_info(kid) {
                    if info.unit != CurrencyUnit::Auth {
                        keyset_units.insert(*kid, info.unit);
                    }
                }
            }
        }

        // Aggregate by unit and build keyset list
        let mut unit_map: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut keyset_totals = Vec::new();

        // Process all keysets sorted by ID for deterministic iteration
        let mut all_keysets: Vec<(Id, CurrencyUnit)> = keyset_units.into_iter().collect();
        all_keysets.sort_by_key(|(id, _)| *id);

        for (kid, unit) in all_keysets {
            if let Some(ref filter_unit) = unit_filter {
                if &unit != filter_unit {
                    continue;
                }
            }

            if let Some(ref filter_id) = keyset_id_filter {
                if &kid != filter_id {
                    continue;
                }
            }

            let issued = total_issued.get(&kid).copied().unwrap_or(Amount::ZERO);
            let redeemed = total_redeemed.get(&kid).copied().unwrap_or(Amount::ZERO);

            let issued_u64 = u64::from(issued);
            let redeemed_u64 = u64::from(redeemed);

            let unit_str = unit.to_string();

            let (u_issued, u_redeemed) = unit_map.entry(unit_str.clone()).or_insert((0, 0));
            *u_issued = u_issued
                .checked_add(issued_u64)
                .ok_or_else(|| Status::internal("Issued total overflow"))?;
            *u_redeemed = u_redeemed
                .checked_add(redeemed_u64)
                .ok_or_else(|| Status::internal("Redeemed total overflow"))?;

            keyset_totals.push(crate::keyset::KeysetTotal {
                keyset_id: kid.to_string(),
                unit: unit_str,
                total_issued: issued_u64,
                total_redeemed: redeemed_u64,
            });
        }

        keyset_totals.sort_by(|a, b| {
            a.unit
                .cmp(&b.unit)
                .then_with(|| a.keyset_id.cmp(&b.keyset_id))
        });

        let unit_totals = unit_map
            .into_iter()
            .map(
                |(unit, (total_issued, total_redeemed))| crate::keyset::UnitTotal {
                    unit,
                    total_issued,
                    total_redeemed,
                },
            )
            .collect();

        Ok(Response::new(crate::keyset::GetKeysetTotalsResponse {
            unit_totals,
            keyset_totals,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::super::test_utils::{create_test_rpc_server, RejectingMutationGuard};
    use super::*;

    async fn record_test_issued_and_redeemed(
        server: &MintRPCServer,
        keyset_id: Id,
        issued_amount: u64,
        redeemed_amount: u64,
    ) {
        use cdk::nuts::{BlindSignature, Proof, SecretKey, State};
        use cdk::secret::Secret;
        use cdk_common::mint::Operation;

        let db = server.mint.localstore();

        if issued_amount > 0 {
            let blinded_message = SecretKey::generate().public_key();
            let sig = BlindSignature {
                amount: Amount::from(issued_amount),
                keyset_id,
                c: SecretKey::generate().public_key(),
                dleq: None,
            };
            let mut tx = db.begin_transaction().await.unwrap();
            tx.add_blind_signatures(&[blinded_message], &[sig], None)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }

        if redeemed_amount > 0 {
            let proof = Proof {
                amount: Amount::from(redeemed_amount),
                keyset_id,
                secret: Secret::generate(),
                c: SecretKey::generate().public_key(),
                witness: None,
                dleq: None,
                p2pk_e: None,
            };
            let mut tx = db.begin_transaction().await.unwrap();
            let mut proofs = tx
                .add_proofs(
                    vec![proof],
                    None,
                    &Operation::new_swap(Amount::ZERO, Amount::ZERO, Amount::ZERO),
                )
                .await
                .unwrap();
            tx.update_proofs_state(&mut proofs, State::Spent)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_keyset_service_rotate_next_keyset() {
        let server = create_test_rpc_server().await;

        let response = KeysetService::rotate_next_keyset(
            &server,
            Request::new(crate::keyset::RotateNextKeysetRequest {
                unit: "sat".to_string(),
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: Some(1),
                use_keyset_v2: Some(true),
                final_expiry: None,
            }),
        )
        .await
        .unwrap();

        let response = response.into_inner();
        assert!(!response.id.is_empty());
        assert_eq!(response.unit, "sat");
        assert_eq!(response.amounts, vec![1, 2, 4, 8]);
        assert_eq!(response.input_fee_ppk, 1);
    }

    #[tokio::test]
    async fn test_keyset_service_get_keyset_totals_non_zero_accounting() {
        let server = create_test_rpc_server().await;

        let sat_keyset_id = server
            .mint
            .keysets()
            .keysets
            .into_iter()
            .find(|k| k.unit == CurrencyUnit::Sat)
            .unwrap()
            .id;

        let usd_response = KeysetService::rotate_next_keyset(
            &server,
            Request::new(crate::keyset::RotateNextKeysetRequest {
                unit: "usd".to_string(),
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: Some(0),
                use_keyset_v2: Some(true),
                final_expiry: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();
        let usd_keyset_id = Id::from_str(&usd_response.id).unwrap();

        record_test_issued_and_redeemed(&server, sat_keyset_id, 600, 200).await;
        record_test_issued_and_redeemed(&server, usd_keyset_id, 50, 20).await;

        let response = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest::default()),
        )
        .await
        .unwrap()
        .into_inner();

        // Check unit totals - units must remain distinct!
        assert_eq!(response.unit_totals.len(), 2);
        let sat_unit = response
            .unit_totals
            .iter()
            .find(|u| u.unit == "sat")
            .unwrap();
        assert_eq!(sat_unit.total_issued, 600);
        assert_eq!(sat_unit.total_redeemed, 200);

        let usd_unit = response
            .unit_totals
            .iter()
            .find(|u| u.unit == "usd")
            .unwrap();
        assert_eq!(usd_unit.total_issued, 50);
        assert_eq!(usd_unit.total_redeemed, 20);

        // Check keyset totals
        assert_eq!(response.keyset_totals.len(), 2);
        let sat_total = response
            .keyset_totals
            .iter()
            .find(|k| k.keyset_id == sat_keyset_id.to_string())
            .unwrap();
        assert_eq!(sat_total.unit, "sat");
        assert_eq!(sat_total.total_issued, 600);
        assert_eq!(sat_total.total_redeemed, 200);

        let usd_total = response
            .keyset_totals
            .iter()
            .find(|k| k.keyset_id == usd_keyset_id.to_string())
            .unwrap();
        assert_eq!(usd_total.unit, "usd");
        assert_eq!(usd_total.total_issued, 50);
        assert_eq!(usd_total.total_redeemed, 20);
    }

    #[tokio::test]
    async fn test_keyset_service_get_keyset_totals_filters() {
        let server = create_test_rpc_server().await;

        let sat_keyset_id = server
            .mint
            .keysets()
            .keysets
            .into_iter()
            .find(|k| k.unit == CurrencyUnit::Sat)
            .unwrap()
            .id;

        let usd_response = KeysetService::rotate_next_keyset(
            &server,
            Request::new(crate::keyset::RotateNextKeysetRequest {
                unit: "usd".to_string(),
                amounts: vec![1, 2, 4, 8],
                input_fee_ppk: Some(0),
                use_keyset_v2: Some(true),
                final_expiry: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();
        let usd_keyset_id = Id::from_str(&usd_response.id).unwrap();

        record_test_issued_and_redeemed(&server, sat_keyset_id, 100, 50).await;
        record_test_issued_and_redeemed(&server, usd_keyset_id, 25, 10).await;

        // Filter by unit "sat"
        let sat_filtered = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest {
                unit: Some("sat".to_string()),
                keyset_id: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(sat_filtered.unit_totals.len(), 1);
        assert_eq!(sat_filtered.unit_totals[0].unit, "sat");
        assert_eq!(sat_filtered.unit_totals[0].total_issued, 100);
        assert_eq!(sat_filtered.unit_totals[0].total_redeemed, 50);
        assert_eq!(sat_filtered.keyset_totals.len(), 1);
        assert_eq!(
            sat_filtered.keyset_totals[0].keyset_id,
            sat_keyset_id.to_string()
        );

        // Filter by keyset_id
        let usd_filtered = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest {
                unit: None,
                keyset_id: Some(usd_keyset_id.to_string()),
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert_eq!(usd_filtered.unit_totals.len(), 1);
        assert_eq!(usd_filtered.unit_totals[0].unit, "usd");
        assert_eq!(usd_filtered.unit_totals[0].total_issued, 25);
        assert_eq!(usd_filtered.unit_totals[0].total_redeemed, 10);
        assert_eq!(usd_filtered.keyset_totals.len(), 1);
        assert_eq!(
            usd_filtered.keyset_totals[0].keyset_id,
            usd_keyset_id.to_string()
        );

        // Filter by unknown unit
        let unknown_unit = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest {
                unit: Some("eur".to_string()),
                keyset_id: None,
            }),
        )
        .await
        .unwrap()
        .into_inner();
        assert!(unknown_unit.unit_totals.is_empty());
        assert!(unknown_unit.keyset_totals.is_empty());
    }

    #[tokio::test]
    async fn test_keyset_service_get_keyset_totals_invalid_arguments() {
        let server = create_test_rpc_server().await;

        // Empty unit
        let err = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest {
                unit: Some("".to_string()),
                keyset_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        // Whitespace unit
        let err = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest {
                unit: Some("   ".to_string()),
                keyset_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        // Empty keyset_id
        let err = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest {
                unit: None,
                keyset_id: Some("".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);

        // Malformed keyset_id
        let err = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest {
                unit: None,
                keyset_id: Some("not-valid-id".to_string()),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn test_keyset_service_get_keyset_totals_read_only_under_mutation_guard() {
        let mut server = create_test_rpc_server().await;
        server.mutation_guard = Some(Arc::new(RejectingMutationGuard));

        let response = KeysetService::get_keyset_totals(
            &server,
            Request::new(crate::keyset::GetKeysetTotalsRequest::default()),
        )
        .await;
        assert!(response.is_ok());
    }
}
