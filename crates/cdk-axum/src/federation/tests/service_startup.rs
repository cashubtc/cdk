//! Federation service startup and worker cancellation.

use super::*;

#[tokio::test]
async fn service_startup_failure_cancels_workers_already_started() {
    let (state, _, _) = federation_route_state().await;
    let tasks = state.federation.tasks.clone();
    let stop = tasks.stop_token();
    let invalid_sync = FederationPeerSyncConfig {
        catch_up_limit: 0,
        ..Default::default()
    };
    let services =
        super::services::FederationServices::new(state.node.clone(), Some(invalid_sync), None);
    assert!(services.start().await.is_err());
    assert!(stop.is_cancelled());
}
