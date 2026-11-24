//! Peer-sync configuration parsing, validation and timing.

use super::*;

#[test]
fn federation_peer_sync_disable_value_accepts_truthy_values() {
    for value in [
        "1", "true", "TRUE", "True", "yes", "YES", "Yes", "on", "ON", "On",
    ] {
        assert!(federation_peer_sync_disabled_value(value));
    }

    for value in ["", "0", "false", "no", "off"] {
        assert!(!federation_peer_sync_disabled_value(value));
    }
}

#[test]
fn federation_peer_sync_config_rejects_invalid_values() {
    let valid = FederationPeerSyncConfig::default();
    let cases = [
        (
            FederationPeerSyncConfig {
                peer_sync_interval: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "peer sync interval",
            },
        ),
        (
            FederationPeerSyncConfig {
                peer_request_timeout: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "peer request timeout",
            },
        ),
        (
            FederationPeerSyncConfig {
                operation_submission_timeout: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "operation submission timeout",
            },
        ),
        (
            FederationPeerSyncConfig {
                catch_up_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "catch-up limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                reconnect_initial_backoff: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "reconnect initial backoff",
            },
        ),
        (
            FederationPeerSyncConfig {
                reconnect_max_backoff: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "reconnect max backoff",
            },
        ),
        (
            FederationPeerSyncConfig {
                reconnect_initial_backoff: Duration::from_secs(2),
                reconnect_max_backoff: Duration::from_secs(1),
                ..valid
            },
            FederationPeerSyncConfigError::ReconnectBackoffOrder,
        ),
        (
            FederationPeerSyncConfig {
                private_rest_inbound_concurrency_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "private REST inbound concurrency limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                private_rest_outbound_concurrency_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "private REST outbound concurrency limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                pending_mempool_item_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "pending mempool item limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                response_waiter_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "response waiter limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                response_cache_limit: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "response cache limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_active_unit_interval: Duration::ZERO,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT active unit interval",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_network_pump_max_messages_per_tick: 0,
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT network pump max messages per tick",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_live_session_max_round: Some(0),
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT live session max round",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_live_session_max_round: Some(9),
                aleph_bft_live_session_rotation_limit: Some(0),
                ..valid
            },
            FederationPeerSyncConfigError::ZeroValue {
                field: "AlephBFT live session rotation limit",
            },
        ),
        (
            FederationPeerSyncConfig {
                aleph_bft_live_session_rotation_limit: Some(2),
                ..valid
            },
            FederationPeerSyncConfigError::RotationLimitRequiresMaxRound,
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(config.validate(), Err(expected));
    }
    assert!(valid.validate().is_ok());
}

#[test]
fn aleph_bft_live_session_options_use_configured_sync_round_cap() {
    let pump_config = CdkAlephBftNetworkPumpConfig::default();
    let default_options =
        FederationAlephBftLiveSessionOptions::from_sync_config(pump_config, Default::default());
    assert_eq!(default_options.max_round(), 3000);
    assert_eq!(default_options.automatic_rotation_limit(), None);

    let configured_options = FederationAlephBftLiveSessionOptions::from_sync_config(
        pump_config,
        FederationPeerSyncConfig {
            aleph_bft_live_session_max_round: Some(9),
            aleph_bft_live_session_rotation_limit: Some(2),
            ..Default::default()
        },
    );
    assert_eq!(configured_options.max_round(), 9);
    assert_eq!(configured_options.automatic_rotation_limit(), Some(2));

    let first_rotated_options = configured_options.after_automatic_rotation();
    assert_eq!(first_rotated_options.max_round(), 9);
    assert_eq!(first_rotated_options.automatic_rotation_limit(), Some(1));

    let second_rotated_options = first_rotated_options.after_automatic_rotation();
    assert_eq!(second_rotated_options.max_round(), 3000);
    assert_eq!(second_rotated_options.automatic_rotation_limit(), None);
}

#[tokio::test(start_paused = true)]
async fn federation_peer_sync_interval_uses_configured_delay() {
    let configured_delay = Duration::from_secs(2);
    let mut interval = federation_peer_sync_interval(FederationPeerSyncConfig {
        peer_sync_interval: configured_delay,
        ..FederationPeerSyncConfig::default()
    });
    interval.tick().await;

    let second_tick = tokio::spawn(async move {
        interval.tick().await;
    });
    tokio::task::yield_now().await;
    assert!(!second_tick.is_finished());

    tokio::time::advance(configured_delay - Duration::from_millis(1)).await;
    tokio::task::yield_now().await;
    assert!(!second_tick.is_finished());

    tokio::time::advance(Duration::from_millis(1)).await;
    second_tick.await.expect("second configured tick");
}

#[test]
fn federation_peer_sync_initial_delay_evenly_phases_members() {
    let interval = Duration::from_secs(5);
    let phases = (0..5)
        .map(|rank| federation_peer_sync_initial_delay(interval, rank, 5))
        .collect::<Vec<_>>();

    assert_eq!(
        phases,
        vec![
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(3),
            Duration::from_secs(4),
        ]
    );
    assert!(phases.iter().all(|phase| *phase < interval));
    assert_eq!(
        federation_peer_sync_initial_delay(interval, 0, 1),
        Duration::ZERO
    );
}

#[test]
fn federation_reconnect_backoff_doubles_until_max() {
    let config = FederationPeerSyncConfig {
        reconnect_initial_backoff: Duration::from_secs(2),
        reconnect_max_backoff: Duration::from_secs(5),
        ..FederationPeerSyncConfig::default()
    };

    assert_eq!(
        federation_reconnect_backoff_delay(config, 0),
        Duration::from_secs(2)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, 1),
        Duration::from_secs(4)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, 2),
        Duration::from_secs(5)
    );
    assert_eq!(
        federation_reconnect_backoff_delay(config, u32::MAX),
        Duration::from_secs(5)
    );
}
