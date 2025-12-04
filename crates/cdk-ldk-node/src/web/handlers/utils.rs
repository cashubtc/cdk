use std::sync::Arc;

use ldk_node::payment::PaymentDirection;
use serde::Deserialize;

use crate::CdkLdkNode;

#[derive(Clone)]
pub struct AppState {
    pub node: Arc<CdkLdkNode>,
}

// Custom deserializer for optional u32 that handles empty strings
pub fn deserialize_optional_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<u32>().map(Some).map_err(serde::de::Error::custom),
    }
}

// Custom deserializer for optional u64 that handles empty strings
pub fn deserialize_optional_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<u64>().map(Some).map_err(serde::de::Error::custom),
    }
}

// Custom deserializer for optional f64 that handles empty strings
pub fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => s.parse::<f64>().map(Some).map_err(serde::de::Error::custom),
    }
}

/// Get paginated payments with efficient filtering and sorting
pub fn get_paginated_payments_streaming(
    node: &ldk_node::Node,
    filter: &str,
    skip: usize,
    take: usize,
) -> (Vec<ldk_node::payment::PaymentDetails>, usize) {
    // Create filter predicate - note LDK expects &&PaymentDetails
    let filter_fn = match filter {
        "incoming" => {
            |p: &&ldk_node::payment::PaymentDetails| p.direction == PaymentDirection::Inbound
        }
        "outgoing" => {
            |p: &&ldk_node::payment::PaymentDetails| p.direction == PaymentDirection::Outbound
        }
        _ => |_: &&ldk_node::payment::PaymentDetails| true,
    };

    // Get filtered payments from LDK
    let filtered_payments = node.list_payments_with_filter(filter_fn);

    // Create sorted index to avoid cloning payments during sort
    let mut time_indexed: Vec<_> = filtered_payments
        .iter()
        .enumerate()
        .map(|(idx, payment)| (payment.latest_update_timestamp, idx))
        .collect();

    // Sort by timestamp (newest first)
    time_indexed.sort_unstable_by(|a, b| b.0.cmp(&a.0));

    let total_count = time_indexed.len();

    // Extract only the payments we need for this page
    let page_payments: Vec<_> = time_indexed
        .into_iter()
        .skip(skip)
        .take(take)
        .map(|(_, idx)| filtered_payments[idx].clone())
        .collect();

    (page_payments, total_count)
}
