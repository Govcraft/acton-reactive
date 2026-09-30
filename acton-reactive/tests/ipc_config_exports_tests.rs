//! Tests for the public import paths of the IPC configuration types.
//!
//! Regression test for issue #10: `RateLimitConfig`, `ShutdownConfig`,
//! `SocketConfig`, `IpcLimitsConfig`, and `IpcTimeoutsConfig` are public
//! fields of `IpcConfig` but previously had no reachable public import
//! path. These tests prove each type can be imported from the public
//! `acton_reactive::ipc` facade and used to construct an `IpcConfig`.
#![cfg(feature = "ipc")]

use acton_reactive::ipc::IpcConfig;
use acton_reactive::ipc::IpcLimitsConfig;
use acton_reactive::ipc::IpcTimeoutsConfig;
use acton_reactive::ipc::RateLimitConfig;
use acton_reactive::ipc::ShutdownConfig;
use acton_reactive::ipc::SocketConfig;

/// Each nested config type is nameable and constructable via its public path.
#[test]
fn config_types_are_constructable_via_public_paths() {
    let socket = SocketConfig {
        app_name: Some("my_app".to_string()),
        ..SocketConfig::default()
    };
    let limits = IpcLimitsConfig {
        max_connections: 42,
        ..IpcLimitsConfig::default()
    };
    let rate_limit = RateLimitConfig {
        enabled: false,
        requests_per_second: 500,
        burst_size: 100,
    };
    let timeouts = IpcTimeoutsConfig {
        request: 10_000,
        ..IpcTimeoutsConfig::default()
    };
    let shutdown = ShutdownConfig {
        drain_timeout: 1_000,
    };

    let config = IpcConfig {
        socket,
        limits,
        rate_limit,
        timeouts,
        shutdown,
    };

    assert_eq!(config.socket.app_name.as_deref(), Some("my_app"));
    assert_eq!(config.limits.max_connections, 42);
    assert!(!config.rate_limit.enabled);
    assert_eq!(config.rate_limit.requests_per_second, 500);
    assert_eq!(config.rate_limit.burst_size, 100);
    assert_eq!(config.timeouts.request, 10_000);
    assert_eq!(config.shutdown.drain_timeout, 1_000);
}

/// Nested fields of an existing `IpcConfig` can be replaced wholesale using
/// the named types, rather than mutating individual fields.
#[test]
fn nested_fields_are_replaceable_with_named_types() {
    let config = IpcConfig {
        rate_limit: RateLimitConfig {
            enabled: true,
            requests_per_second: 250,
            burst_size: 25,
        },
        shutdown: ShutdownConfig { drain_timeout: 750 },
        ..IpcConfig::default()
    };

    assert_eq!(config.rate_limit.requests_per_second, 250);
    assert_eq!(config.shutdown.drain_timeout, 750);
}

/// Old configuration files receive the independent default admission deadline.
#[test]
fn admission_timeout_defaults_and_serialization_are_independent() {
    let config: IpcConfig =
        toml::from_str("[timeouts]\nread_timeout_ms = 0").expect("legacy config");
    assert_eq!(
        config.admission_timeout(),
        Some(std::time::Duration::from_secs(60))
    );
    assert_eq!(config.read_timeout(), None);
    assert_eq!(config.subscription_read_timeout(), None);
    let defaults = IpcConfig::default();
    assert_eq!(
        defaults.read_timeout(),
        Some(std::time::Duration::from_secs(60))
    );
    assert_eq!(
        defaults.admission_timeout(),
        Some(std::time::Duration::from_secs(60))
    );
    let serialized = toml::to_string(&config).expect("serialize");
    assert!(serialized.contains("admission_timeout_ms = 60000"));
    let restored: IpcConfig = toml::from_str(&serialized).expect("roundtrip");
    assert_eq!(restored.admission_timeout(), config.admission_timeout());
    assert_eq!(restored.read_timeout(), None);
}

#[test]
fn disabling_admission_preserves_idle_and_subscription_timeouts() {
    let config: IpcConfig = toml::from_str(
        "[timeouts]\nadmission_timeout_ms = 0\nread_timeout_ms = 42\nsubscription_read_timeout_ms = 75",
    ).expect("independent config");
    assert_eq!(config.admission_timeout(), None);
    assert_eq!(
        config.read_timeout(),
        Some(std::time::Duration::from_millis(42))
    );
    assert_eq!(
        config.subscription_read_timeout(),
        Some(std::time::Duration::from_millis(75))
    );
    let encoded = serde_json::to_value(&config).expect("serialize zero");
    assert_eq!(encoded["timeouts"]["admission_timeout_ms"], 0);
    let restored: IpcConfig = serde_json::from_value(encoded).expect("deserialize zero");
    assert_eq!(restored.admission_timeout(), None);
}
