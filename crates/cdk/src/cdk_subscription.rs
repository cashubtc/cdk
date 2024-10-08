//! Specific Subscription for the cdk crate
use crate::{
    nuts::ProofState,
    subscription::{self, Index, SubId},
};
use serde::{Deserialize, Serialize};

/// Subscription Parameter according to the standard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Params {
    kind: Kind,
    filters: Vec<String>,
    #[serde(rename = "subId")]
    id: SubId,
}

#[derive(Debug, Clone, Copy, Eq, Ord, PartialOrd, PartialEq, Hash, Serialize, Deserialize)]
/// Kind
pub enum Kind {
    ///
    Bolt11MeltQuote,
    ///
    Bolt11MintQuote,
    ///
    ProofState,
}

impl Into<SubId> for &Params {
    fn into(self) -> SubId {
        self.id.clone()
    }
}

impl Into<Vec<Index<(String, Kind)>>> for &Params {
    fn into(self) -> Vec<Index<(String, Kind)>> {
        self.filters
            .iter()
            .map(|filter| Index::from((filter.clone(), self.kind)))
            .collect()
    }
}

/// Manager
pub type Manager = subscription::Manager<ProofState, (String, Kind)>;

#[cfg(test)]
mod test {
    use super::*;
    use std::time::Duration;
    use tokio::time::sleep;

    #[tokio::test]
    async fn active_and_drop() {
        let manager = Manager::default();
        let params = Params {
            kind: Kind::ProofState,
            filters: vec!["x".to_string()],
            id: "uno".into(),
        };

        // Although the same param is used, two subscriptions are created, that
        // is because each index is unique, thanks to `Unique`, it is the
        // responsability of the implementor to make sure that SubId are unique
        // either globally or per client
        let x = vec![
            manager.subscribe(&params, &params).await,
            manager.subscribe(&params, &params).await,
        ];
        assert_eq!(2, manager.active_subscriptions());
        drop(x);

        sleep(Duration::from_millis(10)).await;

        assert_eq!(0, manager.active_subscriptions());
    }
}
