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
