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

//! Tests for issue #5: the listener captures the connecting process's
//! kernel-reported credentials and exposes them for access-control decisions.
//!
//! Both ends of these connections are this test process, so the credentials the
//! listener reports must match this process's own — which is what proves the
//! syscall is genuinely wired rather than returning a plausible constant.

#![cfg(feature = "ipc")]

use std::sync::Arc;
use std::time::Duration;

use acton_reactive::ipc::{
    start_listener, IpcClient, IpcConfig, IpcEnvelope, IpcLimitsConfig, IpcListenerHandle,
    IpcTypeRegistry, SocketConfig,
};
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

/// Start a listener on a private socket with no exposed actors.
async fn start_test_listener(
    socket_path: std::path::PathBuf,
) -> (IpcListenerHandle, CancellationToken) {
    let config = IpcConfig {
        socket: SocketConfig {
            path: Some(socket_path),
            ..SocketConfig::default()
        },
        limits: IpcLimitsConfig::default(),
        ..IpcConfig::default()
    };
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

/// Connect and wait until the server has registered the connection.
async fn connect_and_settle(
    socket: &std::path::Path,
    handle: &IpcListenerHandle,
    expected_active: usize,
) -> IpcClient {
    let client = IpcClient::connect(socket).await.expect("connect");
    client
        .send(IpcEnvelope::new(
            "no_such_actor",
            "NoSuchMessage",
            serde_json::json!({}),
        ))
        .await
        .expect("fire-and-forget send");

    for _ in 0..200 {
        if handle.stats.connections_active() >= expected_active {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("server never registered {expected_active} active connection(s)");
}

/// This process's real uid, obtained without a libc dependency by asking the
/// filesystem who owns a file we just created.
fn own_uid() -> u32 {
    use std::os::unix::fs::MetadataExt as _;

    let file = tempfile::NamedTempFile::new().expect("temp file");
    file.as_file().metadata().expect("metadata").uid()
}

/// The headline behaviour: the listener knows which process connected.
#[tokio::test]
async fn a_connection_reports_the_peer_process_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(socket.clone()).await;

    let _client = connect_and_settle(&socket, &handle, 1).await;

    // Connection ids are handed out from the accepted-connection counter, so the
    // first connection is 1.
    let peer_pid = handle.subscription_manager().peer_pid(1);

    assert_eq!(
        peer_pid,
        Some(std::process::id()),
        "both ends are this process, so the reported pid must be our own"
    );

    cancel.cancel();
    drop(handle);
}

/// uid and gid are captured too — these, not the pid, are the sound basis for an
/// authorization decision.
#[tokio::test]
async fn a_connection_reports_the_peer_user_and_group() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(socket.clone()).await;

    let _client = connect_and_settle(&socket, &handle, 1).await;

    let creds = handle
        .subscription_manager()
        .peer_credentials(1)
        .expect("credentials should be captured on a Unix socket");

    assert_eq!(
        creds.uid(),
        own_uid(),
        "the peer is this process, so the uid must be our own"
    );
    // The rendered form is what shows up in logs.
    assert!(
        creds.to_string().contains(&format!("uid={}", own_uid())),
        "log rendering should carry the uid, got: {creds}"
    );

    cancel.cancel();
    drop(handle);
}

/// Credentials are per-connection, and both connections here are this process.
#[tokio::test]
async fn every_connection_carries_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(socket.clone()).await;

    let _first = connect_and_settle(&socket, &handle, 1).await;
    let _second = connect_and_settle(&socket, &handle, 2).await;

    let manager = handle.subscription_manager();
    assert_eq!(manager.peer_pid(1), Some(std::process::id()));
    assert_eq!(manager.peer_pid(2), Some(std::process::id()));

    cancel.cancel();
    drop(handle);
}

/// A connection that was never established has no credentials to report.
#[tokio::test]
async fn an_unknown_connection_reports_no_credentials() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(socket.clone()).await;

    assert_eq!(handle.subscription_manager().peer_credentials(999), None);
    assert_eq!(handle.subscription_manager().peer_pid(999), None);

    cancel.cancel();
    drop(handle);
}

/// Credentials are gone once the connection ends, so a recycled id cannot
/// inherit the previous peer's identity.
#[tokio::test]
async fn credentials_are_released_when_the_connection_closes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let (handle, cancel) = start_test_listener(socket.clone()).await;

    let client = connect_and_settle(&socket, &handle, 1).await;
    assert!(handle.subscription_manager().peer_credentials(1).is_some());

    client.disconnect().await.expect("clean disconnect");

    for _ in 0..200 {
        if handle.subscription_manager().peer_credentials(1).is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        handle.subscription_manager().peer_credentials(1),
        None,
        "a closed connection must not keep reporting its peer"
    );

    cancel.cancel();
    drop(handle);
}
