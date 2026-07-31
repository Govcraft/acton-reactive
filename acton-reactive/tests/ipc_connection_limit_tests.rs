/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */

//! Tests for issue #14: a connection refused at the listener's limit must reach
//! the client as a typed error, the effective limit must be discoverable at
//! startup and through stats, and the documented config path must actually load.

#![cfg(feature = "ipc")]

use std::sync::Arc;
use std::time::Duration;

use acton_reactive::ipc::{
    start_listener, IpcClient, IpcConfig, IpcEnvelope, IpcError, IpcLimitsConfig, IpcTypeRegistry,
    SocketConfig,
};
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

/// Build a listener config bound to a private socket with the given connection limit.
fn test_config(socket_path: std::path::PathBuf, max_connections: usize) -> IpcConfig {
    IpcConfig {
        socket: SocketConfig {
            path: Some(socket_path),
            ..SocketConfig::default()
        },
        limits: IpcLimitsConfig {
            max_connections,
            ..IpcLimitsConfig::default()
        },
        ..IpcConfig::default()
    }
}

/// Start a listener with no exposed actors, returning its handle and cancel token.
async fn start_test_listener(
    config: IpcConfig,
) -> (acton_reactive::ipc::IpcListenerHandle, CancellationToken) {
    let cancel = CancellationToken::new();
    let handle = start_listener(
        config,
        Arc::new(IpcTypeRegistry::new()),
        Arc::new(DashMap::new()),
        cancel.clone(),
    )
    .await
    .expect("listener should start");
    (handle, cancel)
}

/// An envelope addressed to no exposed actor.
///
/// The point is only to drive the client's request path far enough to observe how
/// the connection fails; whether the actor exists is irrelevant to what is tested.
fn probe_request() -> IpcEnvelope {
    IpcEnvelope::new("no_such_actor", "NoSuchMessage", serde_json::json!({}))
}

/// Connect a client and wait until the server has actually charged it a permit.
///
/// Connecting is not enough: the accept loop takes the permit asynchronously, so
/// without this the next client can race in ahead of the limit being consumed.
async fn connect_and_occupy_a_permit(
    socket: &std::path::Path,
    stats: &acton_reactive::ipc::IpcListenerStats,
    expected_active: usize,
) -> IpcClient {
    let client = IpcClient::connect(socket).await.expect("connect");
    // Fire-and-forget: it needs no reply, it just proves the connection is live.
    client
        .send(probe_request())
        .await
        .expect("fire-and-forget send");

    for _ in 0..200 {
        if stats.connections_active() >= expected_active {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("server never registered {expected_active} active connection(s)");
}

/// The defect from issue #14 and Govcraft/emergent#38.
///
/// A client refused because the server is at its connection limit must learn
/// *that* — not `Broken pipe`, not a bare `ConnectionClosed`. This is the test
/// that fails without the fix.
#[tokio::test]
async fn a_connection_refused_at_the_limit_reports_the_limit_not_a_broken_pipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(test_config(socket.clone(), 1)).await;

    // Occupy the only permit and keep it occupied.
    let _holder = connect_and_occupy_a_permit(&socket, &handle.stats, 1).await;

    // The socket is accepted, so connect() still succeeds — that is the trap.
    let refused = IpcClient::connect(&socket)
        .await
        .expect("connect succeeds; the refusal comes after");

    let error = refused
        .request(probe_request())
        .await
        .expect_err("the server refused this connection");

    assert!(
        matches!(error, IpcError::ConnectionLimitReached { limit: 1 }),
        "expected ConnectionLimitReached {{ limit: 1 }}, got {error:?} ({error})"
    );

    // The message must name the cause an operator can act on.
    let rendered = error.to_string();
    assert!(
        rendered.contains("connection limit"),
        "error message should name the connection limit, got: {rendered}"
    );

    cancel.cancel();
    drop(handle);
}

/// The refusal is also readable directly, without having to issue a request.
#[tokio::test]
async fn a_refused_connection_exposes_its_rejection_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(test_config(socket.clone(), 1)).await;

    let holder = connect_and_occupy_a_permit(&socket, &handle.stats, 1).await;

    let refused = IpcClient::connect(&socket).await.expect("connect");
    // Drive the reader task so it observes the server's rejection frame.
    let _ = refused.request(probe_request()).await;

    assert!(
        matches!(
            refused.rejection_reason(),
            Some(IpcError::ConnectionLimitReached { limit: 1 })
        ),
        "rejection_reason should report the limit, got {:?}",
        refused.rejection_reason()
    );

    // A connection that was accepted normally reports no rejection.
    assert!(
        holder.rejection_reason().is_none(),
        "an accepted connection has no rejection reason"
    );

    cancel.cancel();
    drop(handle);
}

/// An accepted connection is unaffected by the rejection path.
#[tokio::test]
async fn a_connection_within_the_limit_is_not_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(test_config(socket.clone(), 4)).await;

    let client = IpcClient::connect(&socket).await.expect("connect");
    // A short timeout: this request is not expected to be answered, only to prove
    // it does not come back as a refusal.
    let outcome = client
        .request_with_timeout(probe_request(), Duration::from_millis(500))
        .await;

    if let Err(error) = outcome {
        assert!(
            !matches!(error, IpcError::ConnectionLimitReached { .. }),
            "a connection within the limit must not be refused, got {error:?}"
        );
    }
    assert!(
        client.rejection_reason().is_none(),
        "a connection within the limit records no refusal"
    );

    cancel.cancel();
    drop(handle);
}

/// `IpcListenerStats` reports the live count against the configured ceiling, so an
/// embedder can preflight instead of discovering the limit by being refused.
#[tokio::test]
async fn stats_report_the_connection_count_against_the_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(test_config(socket.clone(), 3)).await;

    assert_eq!(handle.stats.max_connections(), 3);
    assert_eq!(handle.stats.connections_active(), 0);
    assert_eq!(handle.stats.connections_available(), 3);

    let _client = connect_and_occupy_a_permit(&socket, &handle.stats, 1).await;

    assert_eq!(handle.stats.max_connections(), 3);
    assert_eq!(handle.stats.connections_active(), 1);
    assert_eq!(
        handle.stats.connections_available(),
        2,
        "one of three permits is in use"
    );

    cancel.cancel();
    drop(handle);
}

/// `connections_available` saturates rather than underflowing.
#[test]
fn available_connections_saturate_at_zero() {
    let stats = acton_reactive::ipc::IpcListenerStats::with_max_connections(0);
    assert_eq!(stats.max_connections(), 0);
    assert_eq!(stats.connections_available(), 0);
}

/// The default ceiling is the raised one. Issue #14: 100 was too low for
/// topologies holding one connection per participant for its process lifetime.
#[test]
fn the_default_connection_limit_is_raised_above_the_value_that_broke_emergent() {
    assert_eq!(IpcLimitsConfig::default().max_connections, 1024);
}

// ============================================================================
// Wire compatibility
// ============================================================================

/// The rejection travels as an ordinary `IpcResponse`, not as a serialized
/// `IpcError`.
///
/// `IpcError` has no `Serialize`/`Deserialize` impl, so the new variant cannot
/// reach the wire as an enum. It is projected onto the existing open string field
/// `error_code` plus a structured payload. A peer that has never heard of
/// `ConnectionLimitReached` therefore still parses the frame — it just reads an
/// error code it does not recognise. This test pins that contract; breaking it
/// would break clients built against 8.x.
#[test]
fn a_rejection_is_wire_compatible_with_a_client_that_does_not_know_the_variant() {
    let response = acton_reactive::ipc::IpcResponse::connection_rejected(100);

    // Exactly what an older peer would do: decode the frame as an IpcResponse
    // and inspect the string fields it has always known about.
    let json = serde_json::to_string(&response).expect("serialize");
    let decoded: serde_json::Value = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(decoded["success"], serde_json::json!(false));
    assert_eq!(
        decoded["error_code"],
        serde_json::json!(acton_reactive::ipc::CONNECTION_LIMIT_REACHED_CODE)
    );
    assert_eq!(
        decoded["correlation_id"],
        serde_json::json!(acton_reactive::ipc::CONNECTION_REJECTED_CORRELATION_ID)
    );
    // The limit is structured data, not something a client must scrape from prose.
    assert_eq!(decoded["payload"]["limit"], serde_json::json!(100));

    // And a current client recovers the typed error from the same bytes.
    let reparsed: acton_reactive::ipc::IpcResponse =
        serde_json::from_str(&json).expect("round-trip");
    assert!(matches!(
        reparsed.as_connection_rejection(),
        Some(IpcError::ConnectionLimitReached { limit: 100 })
    ));
}

/// An ordinary response is never mistaken for a connection-level rejection.
#[test]
fn a_normal_response_is_not_a_rejection() {
    let ok = acton_reactive::ipc::IpcResponse::success("req_123", None);
    assert!(ok.as_connection_rejection().is_none());

    let err = acton_reactive::ipc::IpcResponse::error("req_123", &IpcError::TargetBusy);
    assert!(err.as_connection_rejection().is_none());
}

/// A rejection reason introduced by a newer server still reaches the caller.
///
/// Forward compatibility in the other direction: this client does not need to
/// know the code to report that the server refused it.
#[test]
fn an_unknown_rejection_reason_is_surfaced_rather_than_dropped() {
    let response = acton_reactive::ipc::IpcResponse::error_with_message(
        acton_reactive::ipc::CONNECTION_REJECTED_CORRELATION_ID,
        "SOME_FUTURE_REFUSAL",
        "refused for a reason from the future",
    );

    match response.as_connection_rejection() {
        Some(IpcError::ProtocolError(message)) => {
            assert!(message.contains("from the future"), "got: {message}");
        }
        other => panic!("expected the refusal to survive, got {other:?}"),
    }
}

// ============================================================================
// Configuration path resolution (issue #14, part 3)
// ============================================================================

/// Write an `ipc.toml` declaring `max_connections`, creating parents as needed.
fn write_config(path: &std::path::Path, max_connections: usize) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create config dir");
    }
    std::fs::write(
        path,
        format!("[limits]\nmax_connections = {max_connections}\n"),
    )
    .expect("write config");
}

/// The documented per-application path is now actually read.
///
/// Before the fix the loader only looked at the shared path, so a file placed
/// where the docstring said produced defaults, silently.
#[test]
fn the_loader_finds_a_per_app_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(&dir.path().join("my_app").join("ipc.toml"), 7);

    let config = IpcConfig::load_from_root(dir.path(), "my_app");

    assert_eq!(config.limits.max_connections, 7);
}

/// The pre-existing shared location keeps working, so nobody's file stops loading.
#[test]
fn the_loader_falls_back_to_the_shared_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(&dir.path().join("ipc.toml"), 11);

    let config = IpcConfig::load_from_root(dir.path(), "my_app");

    assert_eq!(config.limits.max_connections, 11);
}

/// Per-application overrides the shared file when both exist.
#[test]
fn a_per_app_config_takes_precedence_over_the_shared_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(&dir.path().join("ipc.toml"), 11);
    write_config(&dir.path().join("my_app").join("ipc.toml"), 7);

    let config = IpcConfig::load_from_root(dir.path(), "my_app");

    assert_eq!(
        config.limits.max_connections, 7,
        "the per-app file must win over the shared one"
    );
}

/// A different application's per-app file must not be picked up.
#[test]
fn another_apps_config_is_not_used() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_config(&dir.path().join("other_app").join("ipc.toml"), 7);

    let config = IpcConfig::load_from_root(dir.path(), "my_app");

    assert_eq!(
        config.limits.max_connections,
        IpcLimitsConfig::default().max_connections,
        "only this app's file or the shared file may apply"
    );
}

/// With neither file present the defaults apply.
#[test]
fn no_config_file_yields_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");

    let config = IpcConfig::load_from_root(dir.path(), "my_app");

    assert_eq!(
        config.limits.max_connections,
        IpcLimitsConfig::default().max_connections
    );
}

/// Timeouts must not be mistaken for refusals: an idle, accepted connection that
/// is never refused reports no rejection reason.
#[tokio::test]
async fn an_idle_connection_records_no_rejection() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(test_config(socket.clone(), 2)).await;

    let client = IpcClient::connect(&socket).await.expect("connect");
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(client.rejection_reason().is_none());

    cancel.cancel();
    drop(handle);
}
