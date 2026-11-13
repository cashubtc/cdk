//! Proofs on an expired keyset are unspendable, so the wallet must hide them
//! from balances and selection while still reconciling them with the mint.
use std::collections::HashMap;
use std::sync::Arc;

use cdk_common::database::WalletDatabase;
use cdk_common::wallet::{ProofInfo, Transaction, TransactionDirection, TransactionStatus};

use crate::nuts::{
    CheckStateResponse, CurrencyUnit, KeySet, KeySetInfo, ProofState, ProofsMethods, PublicKey,
    State,
};
use crate::wallet::test_utils::{
    create_test_db, create_test_wallet_with_mock, make_expired_keyset, make_inactive_keyset,
    test_keyset, test_mint_url, test_proof_info, MockMintConnector,
};
use crate::{Amount, Wallet};

const ACTIVE_AMOUNT: u64 = 8;
const INACTIVE_AMOUNT: u64 = 4;
const EXPIRED_AMOUNT: u64 = 16;

fn info(keyset: &KeySet, active: bool) -> KeySetInfo {
    KeySetInfo {
        id: keyset.id,
        unit: keyset.unit.clone(),
        active,
        input_fee_ppk: keyset.input_fee_ppk,
        final_expiry: keyset.final_expiry,
    }
}

struct Fixture {
    wallet: Wallet,
    client: Arc<MockMintConnector>,
    db: Arc<dyn WalletDatabase<cdk_common::database::Error> + Send + Sync>,
    active: KeySet,
    inactive: KeySet,
    expired: KeySet,
}

impl Fixture {
    /// Builds a wallet holding one unspent proof on an active keyset, one on an
    /// inactive but unexpired keyset, and one on an expired keyset.
    async fn new() -> Self {
        Self::with_expired_keyset(true).await
    }

    async fn with_expired_keyset(include_expired: bool) -> Self {
        let db = create_test_db().await;
        let client = Arc::new(MockMintConnector::new());
        let wallet = create_test_wallet_with_mock(db.clone(), client.clone()).await;

        let active = test_keyset();
        let inactive = make_inactive_keyset();
        let expired = make_expired_keyset();

        let mut keysets = vec![info(&active, true), info(&inactive, false)];
        let mut proofs = vec![
            test_proof_info(active.id, ACTIVE_AMOUNT, test_mint_url()),
            test_proof_info(inactive.id, INACTIVE_AMOUNT, test_mint_url()),
        ];
        if include_expired {
            keysets.push(info(&expired, false));
            proofs.push(test_proof_info(expired.id, EXPIRED_AMOUNT, test_mint_url()));
        }

        db.add_mint(test_mint_url(), None).await.unwrap();
        db.add_mint_keysets(test_mint_url(), keysets).await.unwrap();
        db.update_proofs(proofs, vec![]).await.unwrap();

        Self {
            wallet,
            client,
            db,
            active,
            inactive,
            expired,
        }
    }

    async fn set_state(&self, keyset_id: crate::nuts::Id, state: State) {
        let mut stored: Vec<ProofInfo> = self
            .db
            .get_proofs(None, None, None, None)
            .await
            .unwrap()
            .into_iter()
            .filter(|p| p.proof.keyset_id == keyset_id)
            .collect();
        for proof in stored.iter_mut() {
            proof.state = state;
        }
        self.db.update_proofs(stored, vec![]).await.unwrap();
    }
}

#[tokio::test]
async fn unspent_proofs_exclude_expired_keysets_and_keep_inactive_ones() {
    let fixture = Fixture::new().await;

    let proofs = fixture.wallet.get_unspent_proofs().await.unwrap();

    let ids: Vec<_> = proofs.iter().map(|p| p.keyset_id).collect();
    assert!(ids.contains(&fixture.active.id));
    assert!(ids.contains(&fixture.inactive.id));
    assert!(!ids.contains(&fixture.expired.id));
}

#[tokio::test]
async fn total_balance_excludes_expired_keysets() {
    let fixture = Fixture::new().await;

    let balance = fixture.wallet.total_balance().await.unwrap();

    assert_eq!(balance, Amount::from(ACTIVE_AMOUNT + INACTIVE_AMOUNT));
    assert_eq!(
        balance,
        fixture
            .wallet
            .get_unspent_proofs()
            .await
            .unwrap()
            .total_amount()
            .unwrap()
    );
}

#[tokio::test]
async fn total_balance_sums_everything_when_no_keyset_is_expired() {
    let fixture = Fixture::with_expired_keyset(false).await;

    let balance = fixture.wallet.total_balance().await.unwrap();

    assert_eq!(balance, Amount::from(ACTIVE_AMOUNT + INACTIVE_AMOUNT));
}

#[tokio::test]
async fn total_expired_balance_reports_the_stranded_amount() {
    let fixture = Fixture::new().await;

    assert_eq!(
        fixture.wallet.total_expired_balance().await.unwrap(),
        Amount::from(EXPIRED_AMOUNT)
    );
}

#[tokio::test]
async fn total_expired_balance_is_zero_without_an_expired_keyset() {
    let fixture = Fixture::with_expired_keyset(false).await;

    assert_eq!(
        fixture.wallet.total_expired_balance().await.unwrap(),
        Amount::ZERO
    );
}

#[tokio::test]
async fn get_all_proofs_with_returns_expired_keysets_too() {
    let fixture = Fixture::new().await;

    let proofs = fixture
        .wallet
        .get_all_proofs_with(None, None)
        .await
        .unwrap();

    assert_eq!(proofs.len(), 3);
    assert_eq!(
        proofs.total_amount().unwrap(),
        Amount::from(ACTIVE_AMOUNT + INACTIVE_AMOUNT + EXPIRED_AMOUNT)
    );
}

#[tokio::test]
async fn pending_listings_exclude_expired_keysets() {
    let fixture = Fixture::new().await;
    fixture.set_state(fixture.active.id, State::Pending).await;
    fixture.set_state(fixture.expired.id, State::Pending).await;

    let pending = fixture.wallet.get_pending_proofs().await.unwrap();

    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].keyset_id, fixture.active.id);
    assert_eq!(
        fixture.wallet.total_pending_balance().await.unwrap(),
        Amount::from(ACTIVE_AMOUNT)
    );
    assert_eq!(
        fixture.wallet.total_expired_balance().await.unwrap(),
        Amount::from(EXPIRED_AMOUNT)
    );
}

#[tokio::test]
async fn reserved_listings_exclude_expired_keysets() {
    let fixture = Fixture::new().await;
    fixture.set_state(fixture.expired.id, State::Reserved).await;

    assert!(fixture
        .wallet
        .get_reserved_proofs()
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        fixture.wallet.total_reserved_balance().await.unwrap(),
        Amount::ZERO
    );
    assert_eq!(
        fixture.wallet.total_expired_balance().await.unwrap(),
        Amount::from(EXPIRED_AMOUNT)
    );
}

#[tokio::test]
async fn total_expired_balance_ignores_pending_spent_proofs() {
    let fixture = Fixture::new().await;
    fixture
        .set_state(fixture.expired.id, State::PendingSpent)
        .await;

    assert_eq!(
        fixture.wallet.total_expired_balance().await.unwrap(),
        Amount::ZERO
    );
}

#[tokio::test]
async fn balances_partition_the_stored_proofs() {
    let fixture = Fixture::new().await;
    fixture
        .set_state(fixture.inactive.id, State::Reserved)
        .await;
    fixture.set_state(fixture.expired.id, State::Pending).await;

    let total = fixture.wallet.total_balance().await.unwrap()
        + fixture.wallet.total_pending_balance().await.unwrap()
        + fixture.wallet.total_reserved_balance().await.unwrap()
        + fixture.wallet.total_expired_balance().await.unwrap();

    assert_eq!(
        total,
        Amount::from(ACTIVE_AMOUNT + INACTIVE_AMOUNT + EXPIRED_AMOUNT)
    );
}

#[tokio::test]
async fn a_wallet_with_no_cached_keysets_still_reports_a_balance() {
    let db = create_test_db().await;
    let client = Arc::new(MockMintConnector::new());
    let wallet = create_test_wallet_with_mock(db, client).await;

    assert_eq!(wallet.total_balance().await.unwrap(), Amount::ZERO);
    assert!(wallet.get_unspent_proofs().await.unwrap().is_empty());
    assert_eq!(wallet.total_expired_balance().await.unwrap(), Amount::ZERO);
}

#[tokio::test]
async fn selection_never_sees_expired_proofs() {
    let fixture = Fixture::new().await;

    let available = fixture.wallet.get_unspent_proofs().await.unwrap();
    let selection = Wallet::select_proofs(
        Amount::from(EXPIRED_AMOUNT),
        available,
        &vec![fixture.active.id],
        &[(fixture.active.id, (0, vec![1, 2, 4, 8, 16]).into())]
            .into_iter()
            .collect(),
        false,
    );

    assert!(matches!(selection, Err(crate::Error::InsufficientFunds)));
}

#[tokio::test]
async fn pending_expired_proofs_are_still_reconciled_with_the_mint() {
    let fixture = Fixture::new().await;
    fixture.set_state(fixture.active.id, State::Pending).await;
    fixture.set_state(fixture.expired.id, State::Pending).await;

    let stored = fixture.db.get_proofs(None, None, None, None).await.unwrap();
    let expired_y: PublicKey = stored
        .iter()
        .find(|p| p.proof.keyset_id == fixture.expired.id)
        .map(|p| p.y)
        .unwrap();
    let active_y: PublicKey = stored
        .iter()
        .find(|p| p.proof.keyset_id == fixture.active.id)
        .map(|p| p.y)
        .unwrap();

    fixture
        .client
        .set_check_state_response(Ok(CheckStateResponse {
            states: vec![
                ProofState {
                    y: expired_y,
                    state: State::Spent,
                    witness: None,
                },
                ProofState {
                    y: active_y,
                    state: State::Pending,
                    witness: None,
                },
            ],
        }));

    let remaining = fixture.wallet.check_all_pending_proofs().await.unwrap();

    assert_eq!(remaining, Amount::from(ACTIVE_AMOUNT));
    let left = fixture.db.get_proofs(None, None, None, None).await.unwrap();
    assert!(!left.iter().any(|p| p.y == expired_y));
}

/// `revert_transaction`'s legacy branch reconciles with the mint, so reading
/// through a spendability filter made it no-op on an expired keyset and leave
/// the proofs stuck in `PendingSpent`.
#[tokio::test]
async fn reverting_a_legacy_transaction_reconciles_expired_proofs() {
    let fixture = Fixture::new().await;
    fixture
        .set_state(fixture.expired.id, State::PendingSpent)
        .await;

    let stored = fixture.db.get_proofs(None, None, None, None).await.unwrap();
    let expired = stored
        .iter()
        .find(|p| p.proof.keyset_id == fixture.expired.id)
        .unwrap();

    let transaction = Transaction {
        mint_url: test_mint_url(),
        direction: TransactionDirection::Outgoing,
        amount: Amount::from(EXPIRED_AMOUNT),
        fee: Amount::ZERO,
        unit: CurrencyUnit::Sat,
        ys: vec![expired.y],
        timestamp: 0,
        memo: None,
        metadata: HashMap::new(),
        quote_id: None,
        payment_request: None,
        payment_proof: None,
        payment_method: None,
        saga_id: None,
        status: TransactionStatus::Pending,
    };
    let transaction_id = transaction.id();
    fixture.db.add_transaction(transaction).await.unwrap();

    fixture
        .client
        .set_check_state_response(Ok(CheckStateResponse {
            states: vec![ProofState {
                y: expired.y,
                state: State::Spent,
                witness: None,
            }],
        }));

    fixture
        .wallet
        .revert_transaction(transaction_id)
        .await
        .unwrap();

    let left = fixture.db.get_proofs(None, None, None, None).await.unwrap();
    assert!(
        !left.iter().any(|p| p.y == expired.y),
        "the expired proof the mint reported spent should have been removed"
    );
}
