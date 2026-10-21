use cdk_common::database::WalletDatabase;
use cdk_common::mint_url::MintUrl;
use cdk_sqlite::WalletSqliteDatabase;

#[tokio::test]
async fn persists_with_the_applications_rusqlite() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let path = dir.path().join("wallet.sqlite");
    let mint_url: MintUrl = "https://mint.example".parse().expect("mint URL");

    let db = WalletSqliteDatabase::new(&path)
        .await
        .expect("wallet database");
    db.add_mint(mint_url.clone(), None).await.expect("add mint");
    drop(db);

    let db = WalletSqliteDatabase::new(&path)
        .await
        .expect("reopen wallet");
    assert!(db
        .get_mints()
        .await
        .expect("get mints")
        .contains_key(&mint_url));
    drop(db);

    let connection = rusqlite::Connection::open(&path).expect("application connection");
    let stored_url: String = connection
        .query_row("SELECT mint_url FROM mint", [], |row| row.get(0))
        .expect("persisted mint");
    assert_eq!(stored_url, mint_url.to_string());
}
