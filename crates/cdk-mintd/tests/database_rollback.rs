//! End-to-end tests for the mint database rollback command.

#[cfg(all(feature = "sqlite", not(feature = "sqlcipher")))]
use std::fs;
#[cfg(all(feature = "sqlite", not(feature = "sqlcipher")))]
use std::process::Command;
#[cfg(all(feature = "sqlite", not(feature = "sqlcipher")))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(all(feature = "sqlite", not(feature = "sqlcipher")))]
use cdk_sqlite::MintSqliteDatabase;

#[cfg(all(feature = "sqlite", not(feature = "sqlcipher")))]
#[tokio::test]
async fn database_rollback_command_rolls_back_without_forward_migrating() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after Unix epoch")
        .as_nanos();
    let work_dir = std::env::temp_dir().join(format!(
        "cdk-mintd-database-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&work_dir).expect("create rollback test work directory");
    let db_path = work_dir.join("cdk-mintd.sqlite");

    let db = MintSqliteDatabase::new(&db_path)
        .await
        .expect("apply forward migrations");
    drop(db);

    let output = Command::new(env!("CARGO_BIN_EXE_cdk-mintd"))
        .arg("--work-dir")
        .arg(&work_dir)
        .args(["database", "rollback", "--steps", "3", "--yes"])
        .env_remove("CDK_MINTD_DATABASE")
        .env_remove("CDK_MINTD_WORK_DIR")
        .output()
        .expect("run database rollback command");

    assert!(
        output.status.success(),
        "rollback command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("rollback output is UTF-8");
    assert!(stdout.contains("Rolled back 3 migration(s):"));
    assert!(stdout.contains("20260811000000_add_last_checked_to_mint_quote.sql"));
    assert!(stdout.contains("20260728000000_add_keyset_epoch.sql"));
    assert!(stdout.contains("20260630000000_add_updated_at_to_mint_quote.sql"));
    assert!(stdout.contains("Do not restart this version of cdk-mintd"));

    let error = MintSqliteDatabase::rollback(&db_path, 1)
        .await
        .expect_err("the command must leave the database rolled back");
    assert!(
        error
            .to_string()
            .contains("20260520120000_rename_selected_estimated_blocks_to_fee_index.sql"),
        "unexpected rollback state after command: {error}"
    );

    let db = MintSqliteDatabase::new(&db_path)
        .await
        .expect("normal startup should reapply forward migrations");
    drop(db);
    let rolled_back = MintSqliteDatabase::rollback(&db_path, 3)
        .await
        .expect("reapplied migrations should remain reversible");
    assert_eq!(
        rolled_back,
        vec![
            "20260811000000_add_last_checked_to_mint_quote.sql",
            "20260728000000_add_keyset_epoch.sql",
            "20260630000000_add_updated_at_to_mint_quote.sql",
        ]
    );

    fs::remove_dir_all(work_dir).expect("remove rollback test work directory");
}
