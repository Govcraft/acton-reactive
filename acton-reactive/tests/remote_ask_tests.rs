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

//! Integration tests for `ask` against an actor in another process.
//!
//! Every test here drives a real Unix socket, and every one of them **completes** rather
//! than hanging: that is the property under test as much as any assertion below it. Each
//! gets its own socket in its own temporary directory, so a socket left behind by an
//! earlier run cannot make a later one appear to work.
//!
//! There are no sleeps. `ask` exists to remove them, so the tests wait on `ask` itself and
//! never on a clock.

#![cfg(feature = "ipc")]

use std::path::PathBuf;
use std::time::Duration;

use acton_reactive::ipc::{IpcClient, IpcConfig, IpcListenerHandle};
use acton_reactive::prelude::*;
use acton_test::prelude::*;
use tokio_util::sync::CancellationToken;

/// A deadline short enough to keep the timeout test quick, long enough that a machine
/// under load does not trip it by accident.
const SHORT_TIMEOUT: Duration = Duration::from_millis(500);

#[acton_message(ipc)]
struct GetCount;

#[acton_message(ipc)]
struct Count {
    value: usize,
}

impl Request for GetCount {
    type Response = Count;
}

impl RemoteRequest for GetCount {
    const MESSAGE_TYPE: &'static str = "GetCount";
}

/// A request whose handler deliberately returns without replying.
#[acton_message(ipc)]
struct AskNothing;

impl Request for AskNothing {
    type Response = Count;
}

impl RemoteRequest for AskNothing {
    const MESSAGE_TYPE: &'static str = "AskNothing";
}

/// A request whose handler holds its reply envelope open and never answers.
#[acton_message(ipc)]
struct AskForever;

impl Request for AskForever {
    type Response = Count;
}

impl RemoteRequest for AskForever {
    const MESSAGE_TYPE: &'static str = "AskForever";
}

#[acton_actor]
struct CounterState {
    count: usize,
}

/// What the peer registers, which several tests vary to provoke a specific failure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Registrations {
    /// Everything both sides need.
    Complete,
    /// The reply type is left unregistered, so the peer answers with its fallback blob.
    WithoutReplyType,
    /// The request type is left unregistered, so the peer cannot decode it at all.
    WithoutRequestType,
}

/// Starts a peer process-side runtime exposing a `counter` actor over IPC.
async fn start_server(
    socket_path: PathBuf,
    registrations: Registrations,
    release_token: &CancellationToken,
) -> anyhow::Result<(ActorRuntime, IpcListenerHandle)> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let registry = runtime.ipc_registry();
    if registrations != Registrations::WithoutRequestType {
        registry.register::<GetCount>("GetCount");
    }
    registry.register::<AskNothing>("AskNothing");
    registry.register::<AskForever>("AskForever");
    if registrations != Registrations::WithoutReplyType {
        registry.register::<Count>("Count");
    }

    let mut counter = runtime.new_actor_with_name::<CounterState>("counter".to_string());

    // Seeded before the actor starts, so the reply carries a value no default could
    // produce by accident.
    counter.model.count = 7;

    counter.act_on::<GetCount>(|actor, envelope| {
        let reply = envelope.reply_envelope();
        let value = actor.model.count;
        Reply::pending(async move {
            reply.send(Count { value }).await;
        })
    });

    // Legal, and the common case: a handler that answers nothing at all.
    counter.act_on::<AskNothing>(|_actor, _envelope| Reply::ready());

    // Holds the reply address open without ever using it, which is the one situation
    // closure cannot detect and only a deadline ends. The token lets the test release it
    // at shutdown; a future that never completes would stall the runtime's drain instead,
    // which once made this test take 30s for a reason that had nothing to do with `ask`.
    let handler_token = release_token.clone();
    counter.act_on::<AskForever>(move |_actor, envelope| {
        let reply = envelope.reply_envelope();
        let token = handler_token.clone();
        Reply::pending(async move {
            let _held_open = reply;
            token.cancelled().await;
        })
    });

    let handle = counter.start().await;
    runtime
        .ipc_expose("counter", handle)
        .expect("IPC name should be unclaimed at startup");

    let mut config = IpcConfig::default();
    config.socket.path = Some(socket_path);
    let listener = runtime.start_ipc_listener_with_config(config).await?;

    Ok((runtime, listener))
}

/// Everything a test needs to talk to a peer, and to shut it down again.
struct Peer {
    runtime: ActorRuntime,
    listener: IpcListenerHandle,
    client: IpcClient,
    /// Releases any handler deliberately holding its reply envelope open.
    release_token: CancellationToken,
    _dir: tempfile::TempDir,
}

impl Peer {
    /// Brings up a peer on a socket unique to this test.
    async fn start(registrations: Registrations) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("ipc.sock");
        let release_token = CancellationToken::new();
        let (runtime, listener) =
            start_server(socket_path.clone(), registrations, &release_token).await?;
        let client = IpcClient::connect(&socket_path).await?;

        Ok(Self {
            runtime,
            listener,
            client,
            release_token,
            _dir: dir,
        })
    }

    async fn shutdown(mut self) -> anyhow::Result<()> {
        self.release_token.cancel();
        self.client.disconnect().await?;
        self.listener.stop();
        self.runtime.shutdown_all().await?;
        Ok(())
    }
}

/// The whole point: the same call as a local `ask`, against another process, yielding the
/// reply type the request declares.
#[acton_test]
async fn asking_a_remote_actor_returns_the_declared_reply() -> anyhow::Result<()> {
    let peer = Peer::start(Registrations::Complete).await?;

    let count: Count = peer.client.actor("counter").ask(GetCount).await?;

    assert_eq!(count.value, 7);

    peer.shutdown().await
}

/// A handler that returns without replying is legal, and locally yields `NoReply` in
/// microseconds. The remote path must say the same thing rather than reporting an I/O
/// failure, which is what the peer's overloaded error code would otherwise imply.
#[acton_test]
async fn a_handler_that_never_replies_is_reported_as_no_reply() -> anyhow::Result<()> {
    let peer = Peer::start(Registrations::Complete).await?;

    let outcome = peer.client.actor("counter").ask(AskNothing).await;

    assert_eq!(outcome.err(), Some(AskError::NoReply));

    peer.shutdown().await
}

/// The case closure cannot see: the peer's actor holds a live reply address and simply
/// does not answer. Only a deadline ends this, and it must end promptly rather than hang.
#[acton_test]
async fn a_handler_that_holds_the_reply_open_is_ended_by_the_deadline() -> anyhow::Result<()> {
    let peer = Peer::start(Registrations::Complete).await?;

    let started = std::time::Instant::now();
    let outcome = peer
        .client
        .actor("counter")
        .ask_with_timeout(AskForever, SHORT_TIMEOUT)
        .await;
    let elapsed = started.elapsed();

    assert!(
        matches!(outcome, Err(AskError::TimedOut { .. })),
        "expected TimedOut, got {outcome:?}"
    );

    // The deadline asked for, not the 30s default. Asserting the outcome alone would pass
    // just as happily if the caller's timeout were ignored and `DEFAULT_ASK_TIMEOUT` ran
    // instead — the two are indistinguishable by error value, and only the clock separates
    // them. The bound is loose because it is guarding against 30s, not measuring 500ms.
    assert!(
        elapsed < Duration::from_secs(10),
        "the caller's deadline should govern, but the ask took {elapsed:?}"
    );

    peer.shutdown().await
}

/// A name nobody exposed is a refusal made before dispatch, and the peer's own code is
/// what distinguishes it from every other refusal.
#[acton_test]
async fn asking_an_unknown_actor_is_refused_before_dispatch() -> anyhow::Result<()> {
    let peer = Peer::start(Registrations::Complete).await?;

    let outcome = peer.client.actor("nobody-here").ask(GetCount).await;

    match outcome {
        Err(AskError::PeerRejected { code, .. }) => {
            assert_eq!(code.as_deref(), Some("ACTOR_NOT_FOUND"));
        }
        other => panic!("expected PeerRejected, got {other:?}"),
    }

    peer.shutdown().await
}

/// A request type the peer never registered cannot be decoded there, so nothing runs.
#[acton_test]
async fn an_unregistered_request_type_is_refused_before_dispatch() -> anyhow::Result<()> {
    let peer = Peer::start(Registrations::WithoutRequestType).await?;

    let outcome = peer.client.actor("counter").ask(GetCount).await;

    match outcome {
        Err(AskError::PeerRejected { code, .. }) => {
            assert_eq!(code.as_deref(), Some("UNKNOWN_MESSAGE_TYPE"));
        }
        other => panic!("expected PeerRejected, got {other:?}"),
    }

    peer.shutdown().await
}

/// The peer answers even when it cannot serialize the reply, sending a diagnostic blob
/// instead. That must surface as a reply that is not the declared one — and it must carry
/// the marker, because the only fix is to register the type on the peer.
#[acton_test]
async fn an_unregistered_reply_type_surfaces_as_an_unexpected_reply() -> anyhow::Result<()> {
    let peer = Peer::start(Registrations::WithoutReplyType).await?;

    let outcome = peer.client.actor("counter").ask(GetCount).await;

    match outcome {
        Err(AskError::UnexpectedReply { received, .. }) => {
            assert!(
                received.contains("_ipc_fallback"),
                "the payload should name the peer's fallback, got `{received}`"
            );
        }
        other => panic!("expected UnexpectedReply, got {other:?}"),
    }

    peer.shutdown().await
}

/// The caller's **own** deadline, isolated.
///
/// The test above cannot see it. There the peer is a real listener honouring the
/// `response_timeout_ms` stamped on the envelope, so the peer's clock fires first and the
/// caller's is never reached — ignoring the caller's deadline entirely still passes that
/// test in the same half second. This peer is a bare socket that accepts the connection,
/// reads the request and answers nothing at all, so nothing but the caller's own deadline
/// can end the wait. That is the backstop the no-hang guarantee ultimately rests on.
#[acton_test]
async fn a_peer_that_never_answers_is_ended_by_the_callers_own_deadline() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let socket_path = dir.path().join("silent.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;

    // Accepts and then holds the connection open, reading nothing and writing nothing.
    let accepted = tokio::spawn(async move {
        let (stream, _addr) = listener.accept().await.expect("peer should accept");
        std::future::pending::<()>().await;
        drop(stream);
    });

    let client = IpcClient::connect(&socket_path).await?;

    let started = std::time::Instant::now();
    let outcome = client
        .actor("counter")
        .ask_with_timeout(GetCount, SHORT_TIMEOUT)
        .await;
    let elapsed = started.elapsed();

    assert!(
        matches!(outcome, Err(AskError::TimedOut { .. })),
        "expected TimedOut, got {outcome:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "the caller's deadline should end this, but the ask took {elapsed:?}"
    );

    accepted.abort();
    Ok(())
}

/// With the connection gone, an ask cannot know whether anything was processed, and must
/// say exactly that rather than claiming the request never ran.
#[acton_test]
async fn asking_over_a_closed_connection_reports_uncertain_delivery() -> anyhow::Result<()> {
    let mut peer = Peer::start(Registrations::Complete).await?;

    peer.client.disconnect().await?;

    let outcome = peer.client.actor("counter").ask(GetCount).await;

    assert!(
        matches!(outcome, Err(AskError::TransportFailed { .. })),
        "expected TransportFailed, got {outcome:?}"
    );

    peer.listener.stop();
    peer.runtime.shutdown_all().await?;
    Ok(())
}
