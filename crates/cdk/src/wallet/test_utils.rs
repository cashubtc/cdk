#![cfg(test)]
#![allow(missing_docs)]
#![allow(clippy::missing_panics_doc)]

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use bip39::Mnemonic;
use cdk_common::database::WalletDatabase;
use cdk_common::mint_url::MintUrl;
use cdk_common::nut00::KnownMethod;
use cdk_common::nuts::{
    CheckStateResponse, CurrencyUnit, Id, KeySet, KeySetInfo, KeysetResponse, MeltMethodSettings,
    MintInfo, MintMethodSettings, MintRequest, MintResponse, MintVersion, MppMethodSettings, Proof,
    RestoreResponse, SwapRequest, SwapResponse,
};
use cdk_common::secret::Secret;
use cdk_common::wallet::{MeltQuote, MintQuote};
use cdk_common::{
    Amount, MeltQuoteRequest, MeltQuoteResponse, MeltQuoteState, MintQuoteRequest,
    MintQuoteResponse, SecretKey, State,
};

use crate::nuts::{
    nut17, nut19, BatchCheckMintQuoteRequest, BatchMintRequest, CheckStateRequest,
    MeltQuoteBolt11Response, MeltRequest, MintQuoteBolt11Response, NUT04Settings, NUT05Settings,
    PaymentMethod, RestoreRequest,
};
use crate::wallet::{MintConnector, Wallet};
use crate::Error;

/// Create test database
pub async fn create_test_db() -> Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync>
{
    let db = cdk_sqlite::wallet::memory::empty().await.unwrap();
    Arc::new(db)
}

/// Create a test mint URL
pub fn test_mint_url() -> MintUrl {
    MintUrl::from_str("https://test-mint.example.com").unwrap()
}

/// Create a test keyset ID
pub fn test_keyset_id() -> Id {
    Id::from_str("0094d5a774c40a32").unwrap()
}

/// Create a deterministic test keyset
pub fn test_keyset() -> KeySet {
    let keys = [
        (
            1_u64,
            "0331ad6dbac09400338c17d43fc23d70330547ff7416801c7900735b23fdab189b",
        ),
        (
            2_u64,
            "022d000b4bab1f64ba07c940fdc43ecfe45a94cc574023c3578f17ea635addb437",
        ),
        (
            4_u64,
            "028c127bc4981a995e4159e3df696dcde7c2a0c57f78d3b794080b018af43e2da8",
        ),
        (
            8_u64,
            "023b5e4a60d893d78c8bc7c6d415a13f7538be4898c0567823ed725d5893ae7469",
        ),
        (
            16_u64,
            "02c9b87544bac8bb8169f76671b8d8e107feeece3f6fda71a327311fabbd0a6b08",
        ),
        (
            32_u64,
            "02355039afac742268c9af57b0187957dbf6041f05008a9b2b2f4b3eb863ba81ce",
        ),
        (
            64_u64,
            "0280242434791a53cada80e46009df29c0c7b205d7f460a7917fa7a006e472288b",
        ),
        (
            128_u64,
            "03e231001406fbdbc548ca7edb3d4e5bc7c4dea72280c9caec3a75149f3fea5e49",
        ),
        (
            256_u64,
            "0326ebacc443234661fdc93a1854ab9cb52444a176894f8a54fd6051fa9ff0fbc4",
        ),
        (
            512_u64,
            "02a0eaacbe7b5d08ac5d65bc0f88c4622cfebb9295567e41ccef219454f7b3b880",
        ),
        (
            1024_u64,
            "03a2d67143804905764d8779629ab9d89acb7f1b9b03a14b3721d48892df621edd",
        ),
        (
            2048_u64,
            "031bbc93e01d2c3344ac8d424e5145d5c3b8ec1b309c17faf73beeabeca3c40117",
        ),
        (
            4096_u64,
            "0360e34d975f7fd8b1b89473902cc08752fa611d0f995ad18e7328c0b5e8cdd8f0",
        ),
        (
            8192_u64,
            "02d4b46f83d12b4223754498dc5016bdf761292683229c258f8ce687994879d1b6",
        ),
        (
            16384_u64,
            "03afd259181c3fc0ec5f115decb1cb6e2c5322625fd0c1b24c46bab5cf2f93c3d6",
        ),
        (
            32768_u64,
            "0342baac1eb121da5259a9b8c36a7e68b8454008f9b2f674a666af6f27a9686c98",
        ),
        (
            65536_u64,
            "022a3f9627590870309183cd2399fb6fac32a73302d02c39137c5393e4a5b35b84",
        ),
        (
            131072_u64,
            "033e972225097257f30e1251c84997fabaf0e8e91460e43ad821fe9d4b3d995993",
        ),
        (
            262144_u64,
            "02bcb4a4d34251455d2a76dc2489d34b214cc77c6c1c3c18058b9f1f6ab36016f0",
        ),
        (
            524288_u64,
            "03b301ab3e3104023cde110aa255d9588ad90c0f1e74f952bd0f7639344b99aa4a",
        ),
        (
            1048576_u64,
            "03ad341aebe044eae65ef40aaff4e72c2df21995b00ca494542e437be2acf94e73",
        ),
        (
            2097152_u64,
            "027bd960bc40c3e7767ce76ca3e8391bf982d77c1f1a2cadb7a4c2cc2c03d9dba1",
        ),
        (
            4194304_u64,
            "02bb13a47fc78a80654f19b346bbb21bbe145f127b8db7112b8e5192f6b1cdde21",
        ),
        (
            8388608_u64,
            "037461e11411d64aa7801c4b85d6fb6d2f9ca551f87541ba8723352e01c085d79e",
        ),
        (
            16777216_u64,
            "025a953e3a2314277ff4d19f08d0c21f4db9e03c8c8a6ee0ff65d067205861b909",
        ),
        (
            33554432_u64,
            "02bb9f5004ff60b811d3e4b924b299170a51fa3a02531ea2542c6da3911f35b240",
        ),
        (
            67108864_u64,
            "039de60467c8afda2f24589dd2da3b5e5f979b37e2633e38563161556e9526e583",
        ),
        (
            134217728_u64,
            "03eae2673c65f7f64fb4a2257df98d1ec34677a6f8d12cca46baf5d4dfb900c76d",
        ),
        (
            268435456_u64,
            "03bae4a407997ab35177bd824103941c8b008fae6674493f7ec8072f2e1ab86b33",
        ),
        (
            536870912_u64,
            "032264c60dec079b2e7b2354879973f6b299fc013d4731600581a149108a15ad60",
        ),
        (
            1073741824_u64,
            "02c4b9799e907c1c4e096140587a66ace67d50981a71762a60dc9ef82ac55c74cc",
        ),
        (
            2147483648_u64,
            "03b630c364d6e1f40fa9b3bdb455a4863d50a65cb38b7c0c69735fb683eb77d091",
        ),
    ]
    .into_iter()
    .map(|(amount, public_key)| {
        (
            Amount::from(amount),
            crate::nuts::PublicKey::from_hex(public_key).unwrap(),
        )
    })
    .collect::<BTreeMap<_, _>>();

    KeySet {
        id: test_keyset_id(),
        unit: CurrencyUnit::Sat,
        active: Some(true),
        keys: crate::nuts::Keys::new(keys),
        input_fee_ppk: 101,
        final_expiry: None,
    }
}

/// Create test mint info
pub fn test_mint_info() -> MintInfo {
    MintInfo::new()
        .name("cdk-mintd fake mint")
        .pubkey(
            crate::nuts::PublicKey::from_hex(
                "02836c831cfff541ba17fbca32dd0f6a54d06305893cbcb2af0d7143e66a609147",
            )
            .unwrap(),
        )
        .version(MintVersion::new(
            "cdk-mintd".to_string(),
            "0.15.0-rc.2".to_string(),
        ))
        .description("These are not real sats for testing only")
        .long_description("A longer mint for testing")
        .nuts(
            crate::nuts::Nuts::new()
                .nut04(NUT04Settings::new(
                    vec![
                        MintMethodSettings {
                            method: PaymentMethod::Known(KnownMethod::Bolt11),
                            unit: CurrencyUnit::Sat,
                            min_amount: Some(Amount::from(1_u64)),
                            max_amount: Some(Amount::from(500_000_u64)),
                            options: Some(crate::nuts::nut04::MintMethodOptions::Bolt11 {
                                description: true,
                            }),
                        },
                        MintMethodSettings {
                            method: PaymentMethod::Known(KnownMethod::Bolt12),
                            unit: CurrencyUnit::Sat,
                            min_amount: Some(Amount::from(1_u64)),
                            max_amount: Some(Amount::from(500_000_u64)),
                            options: None,
                        },
                    ],
                    false,
                ))
                .nut05(NUT05Settings {
                    methods: vec![
                        MeltMethodSettings {
                            method: PaymentMethod::Known(KnownMethod::Bolt11),
                            unit: CurrencyUnit::Sat,
                            min_amount: Some(Amount::from(1_u64)),
                            max_amount: Some(Amount::from(500_000_u64)),
                            options: None,
                        },
                        MeltMethodSettings {
                            method: PaymentMethod::Known(KnownMethod::Bolt12),
                            unit: CurrencyUnit::Sat,
                            min_amount: Some(Amount::from(1_u64)),
                            max_amount: Some(Amount::from(500_000_u64)),
                            options: None,
                        },
                    ],
                    disabled: false,
                })
                .nut07(true)
                .nut08(true)
                .nut09(true)
                .nut10(true)
                .nut11(true)
                .nut12(true)
                .nut14(true)
                .nut15(vec![MppMethodSettings {
                    method: PaymentMethod::Known(KnownMethod::Bolt11),
                    unit: CurrencyUnit::Sat,
                }])
                .nut17(vec![
                    nut17::SupportedMethods::new(
                        PaymentMethod::Known(KnownMethod::Bolt11),
                        CurrencyUnit::Sat,
                        vec![
                            nut17::WsCommand::Bolt11MintQuote,
                            nut17::WsCommand::Bolt11MeltQuote,
                            nut17::WsCommand::ProofState,
                        ],
                    ),
                    nut17::SupportedMethods::new(
                        PaymentMethod::Known(KnownMethod::Bolt12),
                        CurrencyUnit::Sat,
                        vec![
                            nut17::WsCommand::Bolt12MintQuote,
                            nut17::WsCommand::Bolt12MeltQuote,
                            nut17::WsCommand::ProofState,
                        ],
                    ),
                ])
                .nut19(
                    Some(60),
                    vec![
                        nut19::CachedEndpoint::new(nut19::Method::Post, nut19::Path::Swap),
                        nut19::CachedEndpoint::new(
                            nut19::Method::Post,
                            nut19::Path::Custom("/v1/mint/bolt11".to_string()),
                        ),
                        nut19::CachedEndpoint::new(
                            nut19::Method::Post,
                            nut19::Path::Custom("/v1/melt/bolt11".to_string()),
                        ),
                        nut19::CachedEndpoint::new(
                            nut19::Method::Post,
                            nut19::Path::Custom("/v1/mint/bolt12".to_string()),
                        ),
                        nut19::CachedEndpoint::new(
                            nut19::Method::Post,
                            nut19::Path::Custom("/v1/melt/bolt12".to_string()),
                        ),
                    ],
                )
                .nut20(true),
        )
        .time(1_773_219_614_u64)
}

/// Create a test proof
pub fn test_proof(keyset_id: Id, amount: u64) -> Proof {
    Proof {
        amount: Amount::from(amount),
        keyset_id,
        secret: Secret::generate(),
        c: SecretKey::generate().public_key(),
        witness: None,
        dleq: None,
        p2pk_e: None,
    }
}

/// Create a test proof info in Unspent state
pub fn test_proof_info(
    keyset_id: Id,
    amount: u64,
    mint_url: MintUrl,
) -> cdk_common::wallet::ProofInfo {
    let proof = test_proof(keyset_id, amount);
    cdk_common::wallet::ProofInfo::new(proof, mint_url, State::Unspent, CurrencyUnit::Sat).unwrap()
}

/// Create a test melt quote
pub fn test_melt_quote() -> MeltQuote {
    MeltQuote {
        id: format!("test_melt_quote_{}", uuid::Uuid::new_v4()),
        mint_url: Some(test_mint_url()),
        unit: CurrencyUnit::Sat,
        amount: Amount::from(1000),
        request: "lnbc1000...".to_string(),
        fee_reserve: Amount::from(10),
        state: MeltQuoteState::Unpaid,
        expiry: 9999999999,
        payment_preimage: None,
        payment_method: PaymentMethod::Known(KnownMethod::Bolt11),
        used_by_operation: None,
        version: 0,
    }
}

/// Create a test mint quote
pub fn test_mint_quote(mint_url: MintUrl) -> MintQuote {
    MintQuote::new(
        format!("test_mint_quote_{}", uuid::Uuid::new_v4()),
        mint_url,
        PaymentMethod::Known(KnownMethod::Bolt11),
        Some(Amount::from(1000)),
        CurrencyUnit::Sat,
        "lnbc1000...".to_string(),
        9999999999,
        None,
    )
}

/// Create a test wallet
pub async fn create_test_wallet(
    db: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync>,
) -> Wallet {
    let mint_url = "https://test-mint.example.com";
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

    Wallet::new(mint_url, CurrencyUnit::Sat, db, seed, None).unwrap()
}

/// Create a test wallet with a mock client
pub async fn create_test_wallet_with_mock(
    db: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync>,
    mock_client: Arc<MockMintConnector>,
) -> Wallet {
    let seed = Mnemonic::generate(12).unwrap().to_seed_normalized("");

    crate::wallet::WalletBuilder::new()
        .mint_url(test_mint_url())
        .unit(CurrencyUnit::Sat)
        .localstore(db)
        .seed(seed)
        .shared_client(mock_client)
        .build()
        .unwrap()
}

/// Mock MintConnector for testing recovery scenarios
#[derive(Debug)]
pub struct MockMintConnector {
    /// Mock mint keyset state
    pub keyset: Mutex<KeySet>,
    /// Mock mint info state
    pub mint_info: Mutex<MintInfo>,
    /// Response for post_check_state calls
    pub check_state_response: Mutex<Option<Result<CheckStateResponse, Error>>>,
    /// Response for post_restore calls
    pub restore_response: Mutex<Option<Result<RestoreResponse, Error>>>,
    /// Response for get_melt_quote_status calls
    pub melt_quote_status_response: Mutex<Option<Result<MeltQuoteBolt11Response<String>, Error>>>,
    /// Response for post_mint calls
    pub post_mint_response: Mutex<Option<Result<MintResponse, Error>>>,
    /// Response for post_swap calls
    pub post_swap_response: Mutex<Option<Result<SwapResponse, Error>>>,
    /// Response for DNS TXT resolution calls
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    pub dns_txt_response: Mutex<Option<Result<Vec<String>, Error>>>,
}

impl Default for MockMintConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl MockMintConnector {
    pub fn new() -> Self {
        let keyset = test_keyset();
        let mint_info = test_mint_info();

        Self {
            keyset: Mutex::new(keyset),
            mint_info: Mutex::new(mint_info),
            check_state_response: Mutex::new(None),
            restore_response: Mutex::new(None),
            melt_quote_status_response: Mutex::new(None),
            post_mint_response: Mutex::new(None),
            post_swap_response: Mutex::new(None),
            #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
            dns_txt_response: Mutex::new(None),
        }
    }

    pub fn set_check_state_response(&self, response: Result<CheckStateResponse, Error>) {
        *self.check_state_response.lock().unwrap() = Some(response);
    }

    pub fn set_mint_keys_response(&self, response: Result<Vec<KeySet>, Error>) {
        match response {
            Ok(mut keysets) => {
                let keyset = keysets
                    .pop()
                    .expect("MockMintConnector: empty keyset response");
                *self.keyset.lock().unwrap() = keyset;
            }
            Err(_) => unimplemented!("error responses for key state are not supported"),
        }
    }

    pub fn set_mint_keyset_response(&self, response: Result<KeySet, Error>) {
        match response {
            Ok(keyset) => *self.keyset.lock().unwrap() = keyset,
            Err(_) => unimplemented!("error responses for key state are not supported"),
        }
    }

    pub fn set_mint_keysets_response(&self, response: Result<KeysetResponse, Error>) {
        match response {
            Ok(keysets) => {
                let keyset_info = keysets
                    .keysets
                    .into_iter()
                    .next()
                    .expect("MockMintConnector: empty keysets response");
                let mut keyset = self.keyset.lock().unwrap();
                keyset.id = keyset_info.id;
                keyset.unit = keyset_info.unit;
                keyset.active = Some(keyset_info.active);
                keyset.input_fee_ppk = keyset_info.input_fee_ppk;
                keyset.final_expiry = keyset_info.final_expiry;
            }
            Err(_) => unimplemented!("error responses for key state are not supported"),
        }
    }

    pub fn set_mint_info_response(&self, response: Result<MintInfo, Error>) {
        match response {
            Ok(mint_info) => *self.mint_info.lock().unwrap() = mint_info,
            Err(_) => unimplemented!("error responses for mint info state are not supported"),
        }
    }

    pub fn set_active_keyset(&self, keyset: KeySet) {
        self.set_mint_keys_response(Ok(vec![keyset.clone()]));
        self.set_mint_keyset_response(Ok(keyset.clone()));
        self.set_mint_keysets_response(Ok(KeysetResponse {
            keysets: vec![KeySetInfo {
                id: keyset.id,
                unit: keyset.unit,
                active: keyset.active.unwrap_or(true),
                input_fee_ppk: keyset.input_fee_ppk,
                final_expiry: keyset.final_expiry,
            }],
        }));
    }

    pub fn reset_default_mint_state(&self) {
        self.set_active_keyset(test_keyset());
        self.set_mint_info_response(Ok(test_mint_info()));
    }

    pub fn _set_restore_response(&self, response: Result<RestoreResponse, Error>) {
        *self.restore_response.lock().unwrap() = Some(response);
    }

    pub fn set_melt_quote_status_response(
        &self,
        response: Result<MeltQuoteBolt11Response<String>, Error>,
    ) {
        *self.melt_quote_status_response.lock().unwrap() = Some(response);
    }

    pub fn set_post_mint_response(&self, response: Result<MintResponse, Error>) {
        *self.post_mint_response.lock().unwrap() = Some(response);
    }

    pub fn set_post_swap_response(&self, response: Result<SwapResponse, Error>) {
        *self.post_swap_response.lock().unwrap() = Some(response);
    }

    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    pub fn set_dns_txt_response(&self, response: Result<Vec<String>, Error>) {
        *self.dns_txt_response.lock().unwrap() = Some(response);
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl MintConnector for MockMintConnector {
    #[cfg(all(feature = "bip353", not(target_arch = "wasm32")))]
    async fn resolve_dns_txt(&self, _domain: &str) -> Result<Vec<String>, Error> {
        self.dns_txt_response
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| Ok(Vec::new()))
    }

    async fn fetch_lnurl_pay_request(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayResponse, Error> {
        unimplemented!()
    }

    async fn fetch_lnurl_invoice(
        &self,
        _url: &str,
    ) -> Result<crate::lightning_address::LnurlPayInvoiceResponse, Error> {
        unimplemented!()
    }

    async fn get_mint_keys(&self) -> Result<Vec<crate::nuts::KeySet>, Error> {
        Ok(vec![self.keyset.lock().unwrap().clone()])
    }

    async fn get_mint_keyset(&self, keyset_id: Id) -> Result<crate::nuts::KeySet, Error> {
        let keyset = self.keyset.lock().unwrap().clone();

        match keyset.id == keyset_id {
            true => Ok(keyset),
            false => Err(Error::UnknownKeySet),
        }
    }

    async fn get_mint_keysets(&self) -> Result<KeysetResponse, Error> {
        let keyset = self.keyset.lock().unwrap().clone();

        Ok(KeysetResponse {
            keysets: vec![KeySetInfo {
                id: keyset.id,
                unit: keyset.unit,
                active: keyset.active.unwrap_or(true),
                input_fee_ppk: keyset.input_fee_ppk,
                final_expiry: keyset.final_expiry,
            }],
        })
    }

    async fn post_mint_quote(
        &self,
        _request: MintQuoteRequest,
    ) -> Result<MintQuoteResponse<String>, Error> {
        unimplemented!()
    }

    async fn get_mint_quote_status(
        &self,
        _method: PaymentMethod,
        _quote_id: &str,
    ) -> Result<MintQuoteResponse<String>, Error> {
        unimplemented!()
    }

    async fn post_mint(
        &self,
        _method: &PaymentMethod,
        _request: MintRequest<String>,
    ) -> Result<MintResponse, Error> {
        self.post_mint_response
            .lock()
            .unwrap()
            .take()
            .expect("MockMintConnector: post_mint called without configured response")
    }

    async fn post_melt_quote(
        &self,
        _request: MeltQuoteRequest,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        unimplemented!()
    }

    async fn get_melt_quote_status(
        &self,
        _method: PaymentMethod,
        _quote_id: &str,
    ) -> Result<MeltQuoteResponse<String>, Error> {
        let response = self
            .melt_quote_status_response
            .lock()
            .unwrap()
            .take()
            .expect(
                "MockMintConnector: get_melt_quote_status called without configured response",
            )?;
        Ok(MeltQuoteResponse::Bolt11(response))
    }

    async fn post_swap(&self, _request: SwapRequest) -> Result<SwapResponse, Error> {
        self.post_swap_response
            .lock()
            .unwrap()
            .take()
            .expect("MockMintConnector: post_swap called without configured response")
    }

    async fn get_mint_info(&self) -> Result<crate::nuts::MintInfo, Error> {
        Ok(self.mint_info.lock().unwrap().clone())
    }

    async fn post_check_state(
        &self,
        _request: CheckStateRequest,
    ) -> Result<CheckStateResponse, Error> {
        self.check_state_response
            .lock()
            .unwrap()
            .take()
            .expect("MockMintConnector: post_check_state called without configured response")
    }

    async fn post_restore(&self, _request: RestoreRequest) -> Result<RestoreResponse, Error> {
        self.restore_response
            .lock()
            .unwrap()
            .take()
            .expect("MockMintConnector: post_restore called without configured response")
    }

    async fn get_auth_wallet(&self) -> Option<crate::wallet::AuthWallet> {
        None
    }

    async fn set_auth_wallet(&self, _wallet: Option<crate::wallet::AuthWallet>) {}

    async fn post_melt(
        &self,
        _method: &PaymentMethod,
        _request: MeltRequest<String>,
    ) -> Result<MeltQuoteBolt11Response<String>, Error> {
        unimplemented!()
    }

    async fn post_batch_check_mint_quote_status(
        &self,
        _method: &PaymentMethod,
        _request: BatchCheckMintQuoteRequest<String>,
    ) -> Result<Vec<MintQuoteBolt11Response<String>>, Error> {
        unimplemented!()
    }

    async fn post_batch_mint(
        &self,
        _method: &PaymentMethod,
        _request: BatchMintRequest<String>,
    ) -> Result<MintResponse, Error> {
        unimplemented!()
    }
}
