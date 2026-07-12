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

//! Integration tests for consuming request-stream responses via [`IpcClient`].
//!
//! Modeled on `examples/ipc_streaming`: a server-side actor streams several
//! frames for a single request, and the client consumes them all through
//! [`IpcClient::request_stream`], observing clean termination on `is_final`.

#![cfg(feature = "ipc")]

use std::path::PathBuf;
use std::time::Duration;

use acton_reactive::ipc::{IpcClient, IpcConfig, IpcEnvelope, IpcListenerHandle};
use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Request to stream `count` ticks back to the client.
#[acton_message(ipc)]
struct CountRequest {
    /// Number of ticks to stream.
    count: u32,
}

/// A single streamed tick.
#[acton_message(ipc)]
struct Tick {
    /// 0-indexed tick number.
    number: u32,
}

/// Streaming counter service state.
#[acton_actor]
struct CounterState;

/// Starts a runtime with a streaming `counter` actor exposed over IPC at the
/// given socket path.
async fn start_streaming_server(
    socket_path: PathBuf,
) -> anyhow::Result<(ActorRuntime, IpcListenerHandle)> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let registry = runtime.ipc_registry();
    registry.register::<CountRequest>("CountRequest");
    registry.register::<Tick>("Tick");

    let mut counter = runtime.new_actor_with_name::<CounterState>("counter".to_string());
    counter.act_on::<CountRequest>(|_actor, envelope| {
        let count = envelope.message().count;
        let reply_envelope = envelope.reply_envelope();

        Reply::pending(async move {
            for number in 0..count {
                reply_envelope.send(Tick { number }).await;
            }
        })
    });
    let handle = counter.start().await;
    runtime.ipc_expose("counter", handle);

    let mut config = IpcConfig::default();
    config.socket.path = Some(socket_path);
    let listener = runtime.start_ipc_listener_with_config(config).await?;

    Ok((runtime, listener))
}

/// The client consumes every stream frame for one request through
/// `request_stream` and sees clean termination on `is_final`.
#[acton_test]
async fn test_request_stream_consumes_all_frames() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let socket_path = dir.path().join("ipc.sock");
    let (mut runtime, listener) = start_streaming_server(socket_path.clone()).await?;

    let client = IpcClient::connect(&socket_path).await?;

    let envelope = IpcEnvelope::new_stream_request(
        "counter",
        "CountRequest",
        serde_json::json!({ "count": 5 }),
    );
    let mut stream_rx = client.request_stream(envelope).await?;

    let mut frames = Vec::new();
    while let Some(frame) = tokio::time::timeout(Duration::from_secs(5), stream_rx.recv())
        .await
        .expect("stream should terminate, not hang")
    {
        frames.push(frame);
    }

    // Five data frames plus the listener's final frame
    assert_eq!(frames.len(), 6);
    for (expected_sequence, frame) in (0_u32..).zip(frames.iter()) {
        assert_eq!(frame.sequence, expected_sequence);
        assert!(frame.error.is_none());
    }

    // The data frames carry the streamed ticks, in order
    for (expected_number, frame) in (0_u32..).zip(frames[..5].iter()) {
        assert!(!frame.is_final);
        let payload = frame.payload.as_ref().expect("data frame should have payload");
        assert_eq!(payload["number"], serde_json::json!(expected_number));
    }

    // The stream terminates cleanly on `is_final`
    let final_frame = frames.last().expect("frames should not be empty");
    assert!(final_frame.is_final);

    client.disconnect().await?;
    listener.stop();
    runtime.shutdown_all().await?;
    Ok(())
}

/// A stream request to an unknown actor terminates with a single final error
/// frame instead of hanging.
#[acton_test]
async fn test_request_stream_unknown_actor_yields_error_frame() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let socket_path = dir.path().join("ipc.sock");
    let (mut runtime, listener) = start_streaming_server(socket_path.clone()).await?;

    let client = IpcClient::connect(&socket_path).await?;

    let envelope = IpcEnvelope::new_stream_request(
        "nonexistent",
        "CountRequest",
        serde_json::json!({ "count": 1 }),
    );
    let mut stream_rx = client.request_stream(envelope).await?;

    let frame = tokio::time::timeout(Duration::from_secs(5), stream_rx.recv())
        .await
        .expect("stream should terminate, not hang")
        .expect("should receive an error frame");

    assert!(frame.is_final);
    let error = frame.error.expect("error frame should carry a message");
    assert!(error.contains("nonexistent"), "unexpected error: {error}");

    // The channel closes after the final frame
    let next = tokio::time::timeout(Duration::from_secs(5), stream_rx.recv())
        .await
        .expect("stream should terminate, not hang");
    assert!(next.is_none());

    client.disconnect().await?;
    listener.stop();
    runtime.shutdown_all().await?;
    Ok(())
}
