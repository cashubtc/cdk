//! ConditionsDatabase tests (NUT-CTF)

use std::str::FromStr;

use cashu::Id;

use crate::database::mint::{ConditionsDatabase, Database, Error};
use crate::mint::{StoredCondition, StoredPartition};

fn test_condition(condition_id: &str) -> StoredCondition {
    StoredCondition {
        condition_id: condition_id.to_string(),
        threshold: 1,
        description: "Test condition".to_string(),
        announcements_json: r#"["deadbeef"]"#.to_string(),
        attestation_status: "pending".to_string(),
        winning_outcome: None,
        attested_at: None,
        created_at: 1000000,
        condition_type: "enum".to_string(),
        lo_bound: None,
        hi_bound: None,
        precision: None,
    }
}

fn test_partition(condition_id: &str) -> StoredPartition {
    StoredPartition {
        condition_id: condition_id.to_string(),
        partition_json: r#"["YES","NO"]"#.to_string(),
        collateral: "sat".to_string(),
        parent_collection_id: "00".repeat(32),
        created_at: 1000000,
    }
}

/// Test add and get a condition
pub async fn add_and_get_condition<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let condition = test_condition("aa".repeat(32).as_str());
    db.add_condition(condition.clone()).await.unwrap();

    let retrieved = db.get_condition(&condition.condition_id).await.unwrap();
    assert!(retrieved.is_some());
    let retrieved = retrieved.unwrap();
    assert_eq!(retrieved.condition_id, condition.condition_id);
    assert_eq!(retrieved.threshold, 1);
    assert_eq!(retrieved.attestation_status, "pending");
    assert!(retrieved.winning_outcome.is_none());
}

/// Test get_condition returns None for nonexistent condition
pub async fn get_nonexistent_condition<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let result = db.get_condition("does_not_exist").await.unwrap();
    assert!(result.is_none());
}

/// Test get_conditions returns multiple conditions
pub async fn get_conditions_multiple<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond1 = test_condition(&"aa".repeat(32));
    let mut cond2 = test_condition(&"bb".repeat(32));
    cond2.description = "Second condition".to_string();

    db.add_condition(cond1).await.unwrap();
    db.add_condition(cond2).await.unwrap();

    let all = db.get_conditions(None, None, &[]).await.unwrap();
    assert_eq!(all.len(), 2);
}

/// Test get_conditions with since filter
pub async fn get_conditions_since<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let mut cond1 = test_condition(&"c1".repeat(32));
    cond1.created_at = 1000;

    let mut cond2 = test_condition(&"c2".repeat(32));
    cond2.created_at = 2000;

    let mut cond3 = test_condition(&"c3".repeat(32));
    cond3.created_at = 3000;

    db.add_condition(cond1).await.unwrap();
    db.add_condition(cond2).await.unwrap();
    db.add_condition(cond3).await.unwrap();

    // No filter returns all
    let all = db.get_conditions(None, None, &[]).await.unwrap();
    assert_eq!(all.len(), 3);

    // since=2000 returns conditions with created_at >= 2000
    let filtered = db.get_conditions(Some(2000), None, &[]).await.unwrap();
    assert_eq!(filtered.len(), 2);

    // since=3000 returns only the latest
    let filtered = db.get_conditions(Some(3000), None, &[]).await.unwrap();
    assert_eq!(filtered.len(), 1);

    // since=4000 returns none
    let filtered = db.get_conditions(Some(4000), None, &[]).await.unwrap();
    assert!(filtered.is_empty());
}

/// Test updating condition attestation status
pub async fn update_condition_attestation<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond = test_condition(&"cc".repeat(32));
    db.add_condition(cond.clone()).await.unwrap();

    db.update_condition_attestation(&cond.condition_id, "attested", Some("YES"), Some(2000000))
        .await
        .unwrap();

    let updated = db
        .get_condition(&cond.condition_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.attestation_status, "attested");
    assert_eq!(updated.winning_outcome, Some("YES".to_string()));
    assert_eq!(updated.attested_at, Some(2000000));
}

/// Test add and get conditional keyset info
pub async fn add_and_get_conditional_keyset_info<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond = test_condition(&"dd".repeat(32));
    db.add_condition(cond.clone()).await.unwrap();

    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    db.add_conditional_keyset_info(
        &cond.condition_id,
        "YES",
        &"ee".repeat(32),
        &keyset_id,
        1000000,
    )
    .await
    .unwrap();

    let keysets = db
        .get_conditional_keysets_for_condition(&cond.condition_id)
        .await
        .unwrap();
    assert_eq!(keysets.len(), 1);
    assert_eq!(keysets.get("YES"), Some(&keyset_id));
}

/// Test multiple outcome collections for the same condition
pub async fn get_conditional_keysets_multiple<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond = test_condition(&"ff".repeat(32));
    db.add_condition(cond.clone()).await.unwrap();

    let ks_yes = Id::from_str("00916bbf7ef91a36").unwrap();
    let ks_no = Id::from_str("009a1f293253e41e").unwrap();

    db.add_conditional_keyset_info(&cond.condition_id, "YES", &"e1".repeat(32), &ks_yes, 1000000)
        .await
        .unwrap();
    db.add_conditional_keyset_info(&cond.condition_id, "NO", &"e2".repeat(32), &ks_no, 1000001)
        .await
        .unwrap();

    let keysets = db
        .get_conditional_keysets_for_condition(&cond.condition_id)
        .await
        .unwrap();
    assert_eq!(keysets.len(), 2);
    assert_eq!(keysets.get("YES"), Some(&ks_yes));
    assert_eq!(keysets.get("NO"), Some(&ks_no));
}

/// Test get_condition_for_keyset lookup by keyset_id
pub async fn get_condition_for_keyset<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond = test_condition(&"a1".repeat(32));
    db.add_condition(cond.clone()).await.unwrap();

    let keyset_id = Id::from_str("00916bbf7ef91a36").unwrap();
    let oc_id = "b1".repeat(32);
    db.add_conditional_keyset_info(&cond.condition_id, "YES", &oc_id, &keyset_id, 1000000)
        .await
        .unwrap();

    let result = db.get_condition_for_keyset(&keyset_id).await.unwrap();
    assert!(result.is_some());
    let (cid, oc, ocid) = result.unwrap();
    assert_eq!(cid, cond.condition_id);
    assert_eq!(oc, "YES");
    assert_eq!(ocid, oc_id);
}

/// Test get_condition_for_keyset returns None for nonexistent keyset
pub async fn get_condition_for_keyset_nonexistent<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let keyset_id = Id::from_str("009a1f293253e41e").unwrap();
    let result = db.get_condition_for_keyset(&keyset_id).await.unwrap();
    assert!(result.is_none());
}

/// Test add and get partitions for a condition
pub async fn add_and_get_partitions<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond = test_condition(&"a2".repeat(32));
    db.add_condition(cond.clone()).await.unwrap();

    let partition = test_partition(&cond.condition_id);
    db.add_partition(partition.clone()).await.unwrap();

    let partitions = db
        .get_partitions_for_condition(&cond.condition_id)
        .await
        .unwrap();
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0].condition_id, cond.condition_id);
    assert_eq!(partitions[0].partition_json, r#"["YES","NO"]"#);
    assert_eq!(partitions[0].collateral, "sat");
    assert_eq!(partitions[0].parent_collection_id, "00".repeat(32));
}

/// Test multiple partitions for the same condition
pub async fn get_partitions_multiple<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond = test_condition(&"a3".repeat(32));
    db.add_condition(cond.clone()).await.unwrap();

    let mut p1 = test_partition(&cond.condition_id);
    p1.partition_json = r#"["YES","NO"]"#.to_string();
    p1.collateral = "sat".to_string();

    let mut p2 = test_partition(&cond.condition_id);
    p2.partition_json = r#"["A|B","C"]"#.to_string();
    p2.collateral = "usd".to_string();
    p2.created_at = 2000000;

    db.add_partition(p1).await.unwrap();
    db.add_partition(p2).await.unwrap();

    let partitions = db
        .get_partitions_for_condition(&cond.condition_id)
        .await
        .unwrap();
    assert_eq!(partitions.len(), 2);
}

/// Test get_partitions returns empty for condition with no partitions
pub async fn get_partitions_empty<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let cond = test_condition(&"a4".repeat(32));
    db.add_condition(cond.clone()).await.unwrap();

    let partitions = db
        .get_partitions_for_condition(&cond.condition_id)
        .await
        .unwrap();
    assert!(partitions.is_empty());
}

/// Test get_conditions with limit parameter
pub async fn get_conditions_limit<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let mut cond1 = test_condition(&"l1".repeat(32));
    cond1.created_at = 1000;
    let mut cond2 = test_condition(&"l2".repeat(32));
    cond2.created_at = 2000;
    let mut cond3 = test_condition(&"l3".repeat(32));
    cond3.created_at = 3000;

    db.add_condition(cond1).await.unwrap();
    db.add_condition(cond2).await.unwrap();
    db.add_condition(cond3).await.unwrap();

    // limit=1 returns only 1 condition
    let limited = db.get_conditions(None, Some(1), &[]).await.unwrap();
    assert_eq!(limited.len(), 1);

    // limit=2 returns 2 conditions
    let limited = db.get_conditions(None, Some(2), &[]).await.unwrap();
    assert_eq!(limited.len(), 2);

    // No limit returns all
    let all = db.get_conditions(None, None, &[]).await.unwrap();
    assert_eq!(all.len(), 3);
}

/// Test get_conditions with status filter
pub async fn get_conditions_status_filter<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let mut cond1 = test_condition(&"s1".repeat(32));
    cond1.created_at = 1000;
    let mut cond2 = test_condition(&"s2".repeat(32));
    cond2.created_at = 2000;

    db.add_condition(cond1.clone()).await.unwrap();
    db.add_condition(cond2.clone()).await.unwrap();

    // Attest second condition
    db.update_condition_attestation(&cond2.condition_id, "attested", Some("YES"), Some(3000))
        .await
        .unwrap();

    // Filter by pending only
    let pending = db
        .get_conditions(None, None, &["pending".to_string()])
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].attestation_status, "pending");

    // Filter by attested only
    let attested = db
        .get_conditions(None, None, &["attested".to_string()])
        .await
        .unwrap();
    assert_eq!(attested.len(), 1);
    assert_eq!(attested[0].attestation_status, "attested");

    // Filter by both returns all
    let both = db
        .get_conditions(
            None,
            None,
            &["pending".to_string(), "attested".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(both.len(), 2);
}

/// Test get_conditions returns results in ascending order by created_at
pub async fn get_conditions_ascending_order<DB>(db: DB)
where
    DB: Database<Error> + ConditionsDatabase<Err = Error>,
{
    let mut cond1 = test_condition(&"o1".repeat(32));
    cond1.created_at = 3000;
    let mut cond2 = test_condition(&"o2".repeat(32));
    cond2.created_at = 1000;
    let mut cond3 = test_condition(&"o3".repeat(32));
    cond3.created_at = 2000;

    // Insert out of order
    db.add_condition(cond1).await.unwrap();
    db.add_condition(cond2).await.unwrap();
    db.add_condition(cond3).await.unwrap();

    let all = db.get_conditions(None, None, &[]).await.unwrap();
    assert_eq!(all.len(), 3);
    assert!(
        all[0].created_at <= all[1].created_at && all[1].created_at <= all[2].created_at,
        "conditions should be sorted ascending by created_at"
    );
}
