//! WASM Database bindings

use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database::WalletDatabase as CdkWalletDatabase;
use cdk_common::wallet::WalletSaga;

use crate::error::WasmError;
use crate::types::*;

/// WASM-compatible wallet database trait with all read and write operations
/// This trait mirrors the CDK WalletDatabase trait structure
#[async_trait::async_trait(?Send)]
pub trait WalletDatabase: 'static {
    // ========== Read methods ==========

    async fn get_mint(&self, mint_url: MintUrl) -> Result<Option<MintInfo>, WasmError>;
    async fn get_mints(&self) -> Result<HashMap<MintUrl, Option<MintInfo>>, WasmError>;
    async fn get_mint_keysets(
        &self,
        mint_url: MintUrl,
    ) -> Result<Option<Vec<KeySetInfo>>, WasmError>;
    async fn get_keyset_by_id(&self, keyset_id: Id) -> Result<Option<KeySetInfo>, WasmError>;
    async fn get_mint_quote(&self, quote_id: String) -> Result<Option<MintQuote>, WasmError>;
    async fn get_mint_quotes(&self) -> Result<Vec<MintQuote>, WasmError>;
    async fn get_unissued_mint_quotes(&self) -> Result<Vec<MintQuote>, WasmError>;
    async fn get_melt_quote(&self, quote_id: String) -> Result<Option<MeltQuote>, WasmError>;
    async fn get_melt_quotes(&self) -> Result<Vec<MeltQuote>, WasmError>;
    async fn get_keys(&self, id: Id) -> Result<Option<Keys>, WasmError>;
    async fn get_proofs(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
        spending_conditions: Option<Vec<SpendingConditions>>,
    ) -> Result<Vec<ProofInfo>, WasmError>;
    async fn get_proofs_by_ys(&self, ys: Vec<PublicKey>) -> Result<Vec<ProofInfo>, WasmError>;
    async fn get_balance(
        &self,
        mint_url: Option<MintUrl>,
        unit: Option<CurrencyUnit>,
        state: Option<Vec<ProofState>>,
    ) -> Result<u64, WasmError>;
    async fn get_transaction(
        &self,
        transaction_id: TransactionId,
    ) -> Result<Option<Transaction>, WasmError>;
    async fn list_transactions(
        &self,
        mint_url: Option<MintUrl>,
        direction: Option<TransactionDirection>,
        unit: Option<CurrencyUnit>,
    ) -> Result<Vec<Transaction>, WasmError>;
    async fn kv_read(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<Option<Vec<u8>>, WasmError>;
    async fn kv_list(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
    ) -> Result<Vec<String>, WasmError>;
    async fn kv_write(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
        value: Vec<u8>,
    ) -> Result<(), WasmError>;
    async fn kv_remove(
        &self,
        primary_namespace: String,
        secondary_namespace: String,
        key: String,
    ) -> Result<(), WasmError>;

    // ========== Write methods ==========

    async fn update_proofs(
        &self,
        added: Vec<ProofInfo>,
        removed_ys: Vec<PublicKey>,
    ) -> Result<(), WasmError>;
    async fn update_proofs_state(
        &self,
        ys: Vec<PublicKey>,
        state: ProofState,
    ) -> Result<(), WasmError>;
    async fn add_transaction(&self, transaction: Transaction) -> Result<(), WasmError>;
    async fn remove_transaction(&self, transaction_id: TransactionId) -> Result<(), WasmError>;
    async fn update_mint_url(
        &self,
        old_mint_url: MintUrl,
        new_mint_url: MintUrl,
    ) -> Result<(), WasmError>;
    async fn increment_keyset_counter(&self, keyset_id: Id, count: u32) -> Result<u32, WasmError>;
    async fn add_mint(
        &self,
        mint_url: MintUrl,
        mint_info: Option<MintInfo>,
    ) -> Result<(), WasmError>;
    async fn remove_mint(&self, mint_url: MintUrl) -> Result<(), WasmError>;
    async fn add_mint_keysets(
        &self,
        mint_url: MintUrl,
        keysets: Vec<KeySetInfo>,
    ) -> Result<(), WasmError>;
    async fn add_mint_quote(&self, quote: MintQuote) -> Result<(), WasmError>;
    async fn remove_mint_quote(&self, quote_id: String) -> Result<(), WasmError>;
    async fn add_melt_quote(&self, quote: MeltQuote) -> Result<(), WasmError>;
    async fn remove_melt_quote(&self, quote_id: String) -> Result<(), WasmError>;
    async fn add_keys(&self, keyset: KeySet) -> Result<(), WasmError>;
    async fn remove_keys(&self, id: Id) -> Result<(), WasmError>;

    // ========== Saga management methods ==========

    async fn add_saga(&self, saga_json: String) -> Result<(), WasmError>;
    async fn get_saga(&self, id: String) -> Result<Option<String>, WasmError>;
    async fn update_saga(&self, saga_json: String) -> Result<bool, WasmError>;
    async fn delete_saga(&self, id: String) -> Result<(), WasmError>;
    async fn get_incomplete_sagas(&self) -> Result<Vec<String>, WasmError>;

    // ========== Proof reservation methods ==========

    async fn reserve_proofs(
        &self,
        ys: Vec<PublicKey>,
        operation_id: String,
    ) -> Result<(), WasmError>;
    async fn release_proofs(&self, operation_id: String) -> Result<(), WasmError>;
    async fn get_reserved_proofs(&self, operation_id: String) -> Result<Vec<ProofInfo>, WasmError>;

    // ========== Quote reservation methods ==========

    async fn reserve_melt_quote(
        &self,
        quote_id: String,
        operation_id: String,
    ) -> Result<(), WasmError>;
    async fn release_melt_quote(&self, operation_id: String) -> Result<(), WasmError>;
    async fn reserve_mint_quote(
        &self,
        quote_id: String,
        operation_id: String,
    ) -> Result<(), WasmError>;
    async fn release_mint_quote(&self, operation_id: String) -> Result<(), WasmError>;
}

/// Internal bridge to convert from the WASM trait to the CDK database trait
pub(crate) struct WalletDatabaseBridge {
    db: Arc<dyn WalletDatabase>,
}

impl WalletDatabaseBridge {
    pub fn new(db: Arc<dyn WalletDatabase>) -> Self {
        Self { db }
    }
}

impl std::fmt::Debug for WalletDatabaseBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WalletDatabaseBridge")
    }
}

#[async_trait::async_trait(?Send)]
impl CdkWalletDatabase<cdk::cdk_database::Error> for WalletDatabaseBridge {
    async fn kv_read(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, cdk::cdk_database::Error> {
        self.db
            .kv_read(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn kv_list(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
    ) -> Result<Vec<String>, cdk::cdk_database::Error> {
        self.db
            .kv_list(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_mint(
        &self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<Option<cdk::nuts::MintInfo>, cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.into();
        let result = self
            .db
            .get_mint(wasm_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(Into::into))
    }

    async fn get_mints(
        &self,
    ) -> Result<
        HashMap<cdk::mint_url::MintUrl, Option<cdk::nuts::MintInfo>>,
        cdk::cdk_database::Error,
    > {
        let result = self
            .db
            .get_mints()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        let mut cdk_result = HashMap::new();
        for (wasm_mint_url, mint_info_opt) in result {
            let cdk_url = wasm_mint_url
                .try_into()
                .map_err(|e: WasmError| cdk::cdk_database::Error::Database(e.to_string().into()))?;
            cdk_result.insert(cdk_url, mint_info_opt.map(Into::into));
        }
        Ok(cdk_result)
    }

    async fn get_mint_keysets(
        &self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<Option<Vec<cdk::nuts::KeySetInfo>>, cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.into();
        let result = self
            .db
            .get_mint_keysets(wasm_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(|keysets| keysets.into_iter().map(Into::into).collect()))
    }

    async fn get_keyset_by_id(
        &self,
        keyset_id: &cdk::nuts::Id,
    ) -> Result<Option<cdk::nuts::KeySetInfo>, cdk::cdk_database::Error> {
        let wasm_id = (*keyset_id).into();
        let result = self
            .db
            .get_keyset_by_id(wasm_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result.map(Into::into))
    }

    async fn get_mint_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MintQuote>, cdk::cdk_database::Error> {
        let result = self
            .db
            .get_mint_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .map(|q| {
                q.try_into().map_err(|e: WasmError| {
                    cdk::cdk_database::Error::Database(e.to_string().into())
                })
            })
            .transpose()?)
    }

    async fn get_mint_quotes(
        &self,
    ) -> Result<Vec<cdk::wallet::MintQuote>, cdk::cdk_database::Error> {
        let result = self
            .db
            .get_mint_quotes()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .into_iter()
            .map(|q| {
                q.try_into().map_err(|e: WasmError| {
                    cdk::cdk_database::Error::Database(e.to_string().into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_unissued_mint_quotes(
        &self,
    ) -> Result<Vec<cdk::wallet::MintQuote>, cdk::cdk_database::Error> {
        let result = self
            .db
            .get_unissued_mint_quotes()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .into_iter()
            .map(|q| {
                q.try_into().map_err(|e: WasmError| {
                    cdk::cdk_database::Error::Database(e.to_string().into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_melt_quote(
        &self,
        quote_id: &str,
    ) -> Result<Option<cdk::wallet::MeltQuote>, cdk::cdk_database::Error> {
        let result = self
            .db
            .get_melt_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .map(|q| {
                q.try_into().map_err(|e: WasmError| {
                    cdk::cdk_database::Error::Database(e.to_string().into())
                })
            })
            .transpose()?)
    }

    async fn get_melt_quotes(
        &self,
    ) -> Result<Vec<cdk::wallet::MeltQuote>, cdk::cdk_database::Error> {
        let result = self
            .db
            .get_melt_quotes()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        Ok(result
            .into_iter()
            .map(|q| {
                q.try_into().map_err(|e: WasmError| {
                    cdk::cdk_database::Error::Database(e.to_string().into())
                })
            })
            .collect::<Result<Vec<_>, _>>()?)
    }

    async fn get_keys(
        &self,
        id: &cdk::nuts::Id,
    ) -> Result<Option<cdk::nuts::Keys>, cdk::cdk_database::Error> {
        let wasm_id: Id = (*id).into();
        let result = self
            .db
            .get_keys(wasm_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        result
            .map(|keys| {
                keys.try_into().map_err(|e: WasmError| {
                    cdk::cdk_database::Error::Database(e.to_string().into())
                })
            })
            .transpose()
    }

    async fn get_proofs(
        &self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        unit: Option<cdk::nuts::CurrencyUnit>,
        state: Option<Vec<cdk::nuts::State>>,
        spending_conditions: Option<Vec<cdk::nuts::SpendingConditions>>,
    ) -> Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.map(Into::into);
        let wasm_unit = unit.map(Into::into);
        let wasm_state = state.map(|s| s.into_iter().map(Into::into).collect());
        let wasm_spending_conditions =
            spending_conditions.map(|sc| sc.into_iter().map(Into::into).collect());

        let result = self
            .db
            .get_proofs(
                wasm_mint_url,
                wasm_unit,
                wasm_state,
                wasm_spending_conditions,
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        let cdk_result: Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> = result
            .into_iter()
            .map(|info| {
                Ok(cdk::types::ProofInfo {
                    proof: info.proof.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    y: info.y.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    mint_url: info.mint_url.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()
                        .map_err(|e: WasmError| {
                            cdk::cdk_database::Error::Database(e.to_string().into())
                        })?,
                    unit: info.unit.into(),
                    used_by_operation: info
                        .used_by_operation
                        .map(|id| uuid::Uuid::parse_str(&id))
                        .transpose()
                        .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?,
                    created_by_operation: info
                        .created_by_operation
                        .map(|id| uuid::Uuid::parse_str(&id))
                        .transpose()
                        .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?,
                })
            })
            .collect();

        cdk_result
    }

    async fn get_proofs_by_ys(
        &self,
        ys: Vec<cdk::nuts::PublicKey>,
    ) -> Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> {
        let wasm_ys: Vec<PublicKey> = ys.into_iter().map(Into::into).collect();

        let result = self
            .db
            .get_proofs_by_ys(wasm_ys)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        let cdk_result: Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> = result
            .into_iter()
            .map(|info| {
                Ok(cdk::types::ProofInfo {
                    proof: info.proof.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    y: info.y.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    mint_url: info.mint_url.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()
                        .map_err(|e: WasmError| {
                            cdk::cdk_database::Error::Database(e.to_string().into())
                        })?,
                    unit: info.unit.into(),
                    used_by_operation: info
                        .used_by_operation
                        .map(|id| uuid::Uuid::parse_str(&id))
                        .transpose()
                        .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?,
                    created_by_operation: info
                        .created_by_operation
                        .map(|id| uuid::Uuid::parse_str(&id))
                        .transpose()
                        .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?,
                })
            })
            .collect();

        cdk_result
    }

    async fn get_balance(
        &self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        unit: Option<cdk::nuts::CurrencyUnit>,
        state: Option<Vec<cdk::nuts::State>>,
    ) -> Result<u64, cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.map(Into::into);
        let wasm_unit = unit.map(Into::into);
        let wasm_state = state.map(|s| s.into_iter().map(Into::into).collect());

        self.db
            .get_balance(wasm_mint_url, wasm_unit, wasm_state)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_transaction(
        &self,
        transaction_id: cdk::wallet::types::TransactionId,
    ) -> Result<Option<cdk::wallet::types::Transaction>, cdk::cdk_database::Error> {
        let wasm_id = transaction_id.into();
        let result = self
            .db
            .get_transaction(wasm_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        result
            .map(|tx| tx.try_into())
            .transpose()
            .map_err(|e: WasmError| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn list_transactions(
        &self,
        mint_url: Option<cdk::mint_url::MintUrl>,
        direction: Option<cdk::wallet::types::TransactionDirection>,
        unit: Option<cdk::nuts::CurrencyUnit>,
    ) -> Result<Vec<cdk::wallet::types::Transaction>, cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.map(Into::into);
        let wasm_direction = direction.map(Into::into);
        let wasm_unit = unit.map(Into::into);

        let result = self
            .db
            .list_transactions(wasm_mint_url, wasm_direction, wasm_unit)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        result
            .into_iter()
            .map(|tx| tx.try_into())
            .collect::<Result<Vec<_>, WasmError>>()
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn update_proofs(
        &self,
        added: Vec<cdk::types::ProofInfo>,
        removed_ys: Vec<cdk::nuts::PublicKey>,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_added: Vec<ProofInfo> = added.into_iter().map(Into::into).collect();
        let wasm_removed_ys: Vec<PublicKey> = removed_ys.into_iter().map(Into::into).collect();
        self.db
            .update_proofs(wasm_added, wasm_removed_ys)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn update_proofs_state(
        &self,
        ys: Vec<cdk::nuts::PublicKey>,
        state: cdk::nuts::State,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_ys: Vec<PublicKey> = ys.into_iter().map(Into::into).collect();
        let wasm_state = state.into();
        self.db
            .update_proofs_state(wasm_ys, wasm_state)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_transaction(
        &self,
        transaction: cdk::wallet::types::Transaction,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_transaction = transaction.into();
        self.db
            .add_transaction(wasm_transaction)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn update_mint_url(
        &self,
        old_mint_url: cdk::mint_url::MintUrl,
        new_mint_url: cdk::mint_url::MintUrl,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_old = old_mint_url.into();
        let wasm_new = new_mint_url.into();
        self.db
            .update_mint_url(wasm_old, wasm_new)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn increment_keyset_counter(
        &self,
        keyset_id: &cdk::nuts::Id,
        count: u32,
    ) -> Result<u32, cdk::cdk_database::Error> {
        let wasm_id = (*keyset_id).into();
        self.db
            .increment_keyset_counter(wasm_id, count)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_mint(
        &self,
        mint_url: cdk::mint_url::MintUrl,
        mint_info: Option<cdk::nuts::MintInfo>,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.into();
        let wasm_mint_info = mint_info.map(Into::into);
        self.db
            .add_mint(wasm_mint_url, wasm_mint_info)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_mint(
        &self,
        mint_url: cdk::mint_url::MintUrl,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.into();
        self.db
            .remove_mint(wasm_mint_url)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_mint_keysets(
        &self,
        mint_url: cdk::mint_url::MintUrl,
        keysets: Vec<cdk::nuts::KeySetInfo>,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_mint_url = mint_url.into();
        let wasm_keysets: Vec<KeySetInfo> = keysets.into_iter().map(Into::into).collect();
        self.db
            .add_mint_keysets(wasm_mint_url, wasm_keysets)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_mint_quote(
        &self,
        quote: cdk::wallet::MintQuote,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_quote = quote.into();
        self.db
            .add_mint_quote(wasm_quote)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_mint_quote(&self, quote_id: &str) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .remove_mint_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_melt_quote(
        &self,
        quote: cdk::wallet::MeltQuote,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_quote = quote.into();
        self.db
            .add_melt_quote(wasm_quote)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_melt_quote(&self, quote_id: &str) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .remove_melt_quote(quote_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_keys(&self, keyset: cdk::nuts::KeySet) -> Result<(), cdk::cdk_database::Error> {
        let wasm_keyset: KeySet = keyset.into();
        self.db
            .add_keys(wasm_keyset)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_keys(&self, id: &cdk::nuts::Id) -> Result<(), cdk::cdk_database::Error> {
        let wasm_id = (*id).into();
        self.db
            .remove_keys(wasm_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn remove_transaction(
        &self,
        transaction_id: cdk::wallet::types::TransactionId,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_id = transaction_id.into();
        self.db
            .remove_transaction(wasm_id)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn add_saga(&self, saga: WalletSaga) -> Result<(), cdk::cdk_database::Error> {
        let json = serde_json::to_string(&saga)
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        self.db
            .add_saga(json)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_saga(
        &self,
        id: &uuid::Uuid,
    ) -> Result<Option<WalletSaga>, cdk::cdk_database::Error> {
        let json_opt = self
            .db
            .get_saga(id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        match json_opt {
            Some(json) => {
                let saga: WalletSaga = serde_json::from_str(&json)
                    .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
                Ok(Some(saga))
            }
            None => Ok(None),
        }
    }

    async fn update_saga(&self, saga: WalletSaga) -> Result<bool, cdk::cdk_database::Error> {
        let json = serde_json::to_string(&saga)
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;
        self.db
            .update_saga(json)
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn delete_saga(&self, id: &uuid::Uuid) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .delete_saga(id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_incomplete_sagas(&self) -> Result<Vec<WalletSaga>, cdk::cdk_database::Error> {
        let json_vec = self
            .db
            .get_incomplete_sagas()
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        json_vec
            .into_iter()
            .map(|json| {
                serde_json::from_str(&json)
                    .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
            })
            .collect()
    }

    async fn reserve_proofs(
        &self,
        ys: Vec<cdk::nuts::PublicKey>,
        operation_id: &uuid::Uuid,
    ) -> Result<(), cdk::cdk_database::Error> {
        let wasm_ys: Vec<PublicKey> = ys.into_iter().map(Into::into).collect();
        self.db
            .reserve_proofs(wasm_ys, operation_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn release_proofs(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .release_proofs(operation_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn get_reserved_proofs(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<Vec<cdk::types::ProofInfo>, cdk::cdk_database::Error> {
        let result = self
            .db
            .get_reserved_proofs(operation_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?;

        result
            .into_iter()
            .map(|info| {
                Ok(cdk::types::ProofInfo {
                    proof: info.proof.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    y: info.y.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    mint_url: info.mint_url.try_into().map_err(|e: WasmError| {
                        cdk::cdk_database::Error::Database(e.to_string().into())
                    })?,
                    state: info.state.into(),
                    spending_condition: info
                        .spending_condition
                        .map(|sc| sc.try_into())
                        .transpose()
                        .map_err(|e: WasmError| {
                            cdk::cdk_database::Error::Database(e.to_string().into())
                        })?,
                    unit: info.unit.into(),
                    used_by_operation: info
                        .used_by_operation
                        .map(|id| uuid::Uuid::parse_str(&id))
                        .transpose()
                        .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?,
                    created_by_operation: info
                        .created_by_operation
                        .map(|id| uuid::Uuid::parse_str(&id))
                        .transpose()
                        .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))?,
                })
            })
            .collect()
    }

    async fn reserve_melt_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .reserve_melt_quote(quote_id.to_string(), operation_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn release_melt_quote(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .release_melt_quote(operation_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn reserve_mint_quote(
        &self,
        quote_id: &str,
        operation_id: &uuid::Uuid,
    ) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .reserve_mint_quote(quote_id.to_string(), operation_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn release_mint_quote(
        &self,
        operation_id: &uuid::Uuid,
    ) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .release_mint_quote(operation_id.to_string())
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn kv_write(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
        value: &[u8],
    ) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .kv_write(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
                value.to_vec(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }

    async fn kv_remove(
        &self,
        primary_namespace: &str,
        secondary_namespace: &str,
        key: &str,
    ) -> Result<(), cdk::cdk_database::Error> {
        self.db
            .kv_remove(
                primary_namespace.to_string(),
                secondary_namespace.to_string(),
                key.to_string(),
            )
            .await
            .map_err(|e| cdk::cdk_database::Error::Database(e.to_string().into()))
    }
}

/// Helper function to create a CDK database from the WASM trait
pub fn create_cdk_database_from_wasm(
    db: Arc<dyn WalletDatabase>,
) -> Arc<dyn CdkWalletDatabase<cdk::cdk_database::Error>> {
    Arc::new(WalletDatabaseBridge::new(db))
}
