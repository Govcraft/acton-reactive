//! Policy enforcement through real IPC sockets and actor dispatch.
#![cfg(feature = "ipc")]

use std::sync::Arc;
use std::time::Duration;

use acton_reactive::ipc::protocol::{
    read_frame, write_frame, Format, MAX_FRAME_SIZE, MSG_TYPE_DISCOVER, MSG_TYPE_REQUEST,
    MSG_TYPE_SUBSCRIBE, MSG_TYPE_SUBSCRIBE_PATTERNS, MSG_TYPE_UNSUBSCRIBE,
    MSG_TYPE_UNSUBSCRIBE_PATTERNS,
};
use acton_reactive::ipc::{
    start_listener_with_policy, IpcAccessDenied, IpcAdmission, IpcConfig, IpcConnectionContext,
    IpcConnectionInfo, IpcEnvelope, IpcIdentity, IpcListenerHandle, IpcOperation,
    IpcSecurityPolicy, IpcTypeRegistry, IPC_ACCESS_DENIED_CODE,
};
use acton_reactive::prelude::*;
use dashmap::DashMap;
use serde_json::{json, Value};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;

struct Policy {
    reject_admission: bool,
    deny_operations: bool,
}
impl IpcSecurityPolicy for Policy {
    fn admit(&self, info: IpcConnectionInfo) -> IpcAdmission<'_> {
        Box::pin(async move {
            if self.reject_admission {
                return Err(IpcAccessDenied::new("Unrecognized primitive"));
            }
            assert!(info.peer_credentials().is_some());
            Ok(IpcIdentity::new(format!(
                "primitive-{}",
                info.connection_id()
            )))
        })
    }
    fn authorize(
        &self,
        context: &IpcConnectionContext,
        _operation: IpcOperation<'_>,
    ) -> Result<(), IpcAccessDenied> {
        assert!(context.identity::<String>().is_some());
        if self.deny_operations {
            Err(IpcAccessDenied::new("Capability missing"))
        } else {
            Ok(())
        }
    }
}

fn formats() -> Vec<Format> {
    #[cfg(feature = "ipc-messagepack")]
    {
        vec![Format::Json, Format::MessagePack]
    }
    #[cfg(not(feature = "ipc-messagepack"))]
    {
        vec![Format::Json]
    }
}

async fn listener(path: &std::path::Path, policy: Arc<dyn IpcSecurityPolicy>) -> IpcListenerHandle {
    let mut config = IpcConfig::default();
    config.socket.path = Some(path.to_path_buf());
    start_listener_with_policy(
        config,
        Arc::new(IpcTypeRegistry::new()),
        Arc::new(DashMap::new()),
        CancellationToken::new(),
        policy,
    )
    .await
    .expect("start listener")
}

async fn response(stream: &mut UnixStream) -> Value {
    let (_, format, bytes) =
        tokio::time::timeout(Duration::from_secs(5), read_frame(stream, MAX_FRAME_SIZE))
            .await
            .expect("response deadline")
            .expect("response frame");
    format.deserialize(&bytes).expect("decode response")
}

async fn send(stream: &mut UnixStream, format: Format, operation: u8, payload: &Value) -> Value {
    write_frame(
        stream,
        operation,
        format,
        &format.serialize(payload).expect("encode"),
    )
    .await
    .expect("write");
    response(stream).await
}

#[tokio::test]
async fn admission_rejection_precedes_all_client_frames() {
    for format in formats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("ipc.sock");
        let handle = listener(
            &socket,
            Arc::new(Policy {
                reject_admission: true,
                deny_operations: false,
            }),
        )
        .await;
        let mut stream = UnixStream::connect(&socket).await.expect("connect");
        let request = json!({"correlation_id":"try", "message_types":["Secret"]});
        let _ = write_frame(
            &mut stream,
            MSG_TYPE_SUBSCRIBE,
            format,
            &format.serialize(&request).expect("encode"),
        )
        .await;
        let rejection = response(&mut stream).await;
        assert_eq!(rejection["error_code"], IPC_ACCESS_DENIED_CODE);
        assert_eq!(rejection["success"], false);
        assert_eq!(handle.subscription_manager().connection_count(), 0);
        assert!(tokio::time::timeout(
            Duration::from_secs(2),
            read_frame(&mut stream, MAX_FRAME_SIZE)
        )
        .await
        .expect("closed deadline")
        .is_err());
        handle.stop();
    }
}

#[tokio::test]
async fn authorization_covers_raw_requests_subscriptions_patterns_and_discovery() {
    for format in formats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("ipc.sock");
        let handle = listener(
            &socket,
            Arc::new(Policy {
                reject_admission: false,
                deny_operations: true,
            }),
        )
        .await;
        let mut stream = UnixStream::connect(&socket).await.expect("connect");
        for operation in [
            MSG_TYPE_SUBSCRIBE,
            MSG_TYPE_UNSUBSCRIBE,
            MSG_TYPE_SUBSCRIBE_PATTERNS,
            MSG_TYPE_UNSUBSCRIBE_PATTERNS,
            MSG_TYPE_DISCOVER,
        ] {
            let denied = send(&mut stream, format, operation, &json!({"correlation_id":"denied", "message_types":["Secret"],"patterns":["*"],"include_actors":true,"include_message_types":true})).await;
            assert_eq!(denied["correlation_id"], "denied");
            assert_eq!(denied["success"], false);
            assert!(denied["error"]
                .as_str()
                .expect("error")
                .contains(IPC_ACCESS_DENIED_CODE));
        }
        for (reply, stream_request) in [(false, false), (true, false), (false, true)] {
            let mut envelope =
                IpcEnvelope::new("missing", "Unregistered", json!({"identity":"forged"}));
            envelope.expects_reply = reply;
            envelope.expects_stream = stream_request;
            let denied = send(
                &mut stream,
                format,
                MSG_TYPE_REQUEST,
                &serde_json::to_value(envelope).expect("envelope"),
            )
            .await;
            assert_eq!(
                denied["error_code"], IPC_ACCESS_DENIED_CODE,
                "authorization must precede target/type lookup"
            );
            if stream_request {
                assert_eq!(denied["is_final"], true);
            } else {
                assert_eq!(denied["success"], false);
            }
        }
        assert_eq!(handle.subscription_manager().total_subscriptions(), 0);
        assert_eq!(handle.stats.in_flight_requests(), 0);
        handle.stop();
    }
}

#[acton_message(ipc)]
struct IdentityProbe {
    identity: String,
}
#[acton_message(ipc)]
struct ProbeReply {
    connection_id: usize,
}
#[acton_actor]
struct ProbeState;

#[tokio::test]
async fn trusted_identity_survives_fire_and_forget_reply_and_stream_dispatch() {
    for format in formats() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("ipc.sock");
        let mut runtime = ActonApp::launch_async().await;
        runtime
            .ipc_registry()
            .register::<IdentityProbe>("IdentityProbe");
        runtime.ipc_registry().register::<ProbeReply>("ProbeReply");
        let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
        let mut actor = runtime.new_actor::<ProbeState>();
        actor.act_on::<IdentityProbe>(move |_, envelope| {
            let context = envelope.ipc_context().expect("trusted context");
            assert_eq!(envelope.message().identity, "forged");
            let connection_id = context.connection_id();
            assert_eq!(
                context.identity::<String>().expect("identity"),
                &format!("primitive-{connection_id}")
            );
            seen_tx.send(context.clone()).expect("observer");
            let reply = envelope.reply_envelope();
            Reply::pending(async move {
                reply.send(ProbeReply { connection_id }).await;
            })
        });
        runtime
            .ipc_expose("probe", actor.start().await)
            .expect("expose");
        let mut config = IpcConfig::default();
        config.socket.path = Some(socket.clone());
        let listener = runtime
            .start_ipc_listener_with_policy(
                config,
                Arc::new(Policy {
                    reject_admission: false,
                    deny_operations: false,
                }),
            )
            .await
            .expect("listener");
        for (reply, stream_request) in [(false, false), (true, false), (false, true)] {
            let mut stream = UnixStream::connect(&socket).await.expect("connect");
            let mut envelope =
                IpcEnvelope::new("probe", "IdentityProbe", json!({"identity":"forged"}));
            envelope.expects_reply = reply;
            envelope.expects_stream = stream_request;
            let received = send(
                &mut stream,
                format,
                MSG_TYPE_REQUEST,
                &serde_json::to_value(envelope).expect("envelope"),
            )
            .await;
            assert!(received["error"].is_null());
            let context = tokio::time::timeout(Duration::from_secs(2), seen_rx.recv())
                .await
                .expect("dispatch deadline")
                .expect("context");
            assert!(listener.revoke_connection(context.connection_id()));
            assert!(context.is_revoked());
        }
        listener.stop();
        runtime.shutdown_all().await.expect("shutdown");
    }
}

struct SlowAdmission {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
impl IpcSecurityPolicy for SlowAdmission {
    fn admit(&self, info: IpcConnectionInfo) -> IpcAdmission<'_> {
        Box::pin(async move {
            if info.connection_id() == 1 {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(IpcIdentity::new(info.connection_id()))
        })
    }
    fn authorize(
        &self,
        _: &IpcConnectionContext,
        _: IpcOperation<'_>,
    ) -> Result<(), IpcAccessDenied> {
        Ok(())
    }
}

#[tokio::test]
async fn slow_admission_does_not_block_other_connections_and_shutdown_cancels_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let entered = Arc::new(Notify::new());
    let handle = listener(
        &socket,
        Arc::new(SlowAdmission {
            entered: entered.clone(),
            release: Arc::new(Notify::new()),
        }),
    )
    .await;
    let mut first = UnixStream::connect(&socket).await.expect("first");
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .expect("admission entered");
    let mut second = UnixStream::connect(&socket).await.expect("second");
    let accepted = send(
        &mut second,
        Format::Json,
        MSG_TYPE_SUBSCRIBE,
        &json!({"correlation_id":"second","message_types":["Event"]}),
    )
    .await;
    assert_eq!(accepted["success"], true);
    assert!(handle
        .subscription_manager()
        .connection_context(1)
        .is_none());
    handle.stop();
    assert!(tokio::time::timeout(
        Duration::from_secs(2),
        read_frame(&mut first, MAX_FRAME_SIZE)
    )
    .await
    .expect("shutdown cancels admission")
    .is_err());
}

#[tokio::test]
async fn revocation_closes_waiting_request_and_stream_and_releases_in_flight_count() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let mut runtime = ActonApp::launch_async().await;
    runtime
        .ipc_registry()
        .register::<IdentityProbe>("IdentityProbe");
    runtime.ipc_registry().register::<ProbeReply>("ProbeReply");
    let (seen_tx, mut seen_rx) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let actor_release = release.clone();
    let mut actor = runtime.new_actor::<ProbeState>();
    actor.act_on::<IdentityProbe>(move |_, envelope| {
        let context = envelope.ipc_context().expect("context").clone();
        let release = actor_release.clone();
        let seen = seen_tx.clone();
        let reply = envelope.reply_envelope();
        Reply::pending(async move {
            seen.send(context).expect("observer");
            release.notified().await;
            reply.send(ProbeReply { connection_id: 0 }).await;
        })
    });
    runtime
        .ipc_expose("probe", actor.start().await)
        .expect("expose");
    let mut config = IpcConfig::default();
    config.socket.path = Some(socket.clone());
    let handle = runtime
        .start_ipc_listener_with_policy(
            config,
            Arc::new(Policy {
                reject_admission: false,
                deny_operations: false,
            }),
        )
        .await
        .expect("listener");
    for stream_request in [false, true] {
        let mut stream = UnixStream::connect(&socket).await.expect("connect");
        let mut envelope = IpcEnvelope::new("probe", "IdentityProbe", json!({"identity":"forged"}));
        envelope.expects_reply = !stream_request;
        envelope.expects_stream = stream_request;
        write_frame(
            &mut stream,
            MSG_TYPE_REQUEST,
            Format::Json,
            &Format::Json.serialize(&envelope).expect("encode"),
        )
        .await
        .expect("write");
        let context = tokio::time::timeout(Duration::from_secs(2), seen_rx.recv())
            .await
            .expect("dispatch deadline")
            .expect("context");
        assert_eq!(handle.stats.in_flight_requests(), 1);
        assert!(handle.revoke_connection(context.connection_id()));
        assert!(tokio::time::timeout(
            Duration::from_secs(2),
            read_frame(&mut stream, MAX_FRAME_SIZE)
        )
        .await
        .expect("revoked socket deadline")
        .is_err());
        assert!(context.is_revoked());
        tokio::time::timeout(Duration::from_secs(2), async {
            while handle.stats.in_flight_requests() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("in-flight accounting cleared");
        release.notify_one();
    }
    handle.stop();
    runtime.shutdown_all().await.expect("shutdown");
}

#[tokio::test]
async fn admission_timeout_releases_connection_capacity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let mut config = IpcConfig::default();
    config.socket.path = Some(socket.clone());
    config.timeouts.read = 30;
    config.limits.max_connections = 1;
    let policy = Arc::new(SlowAdmission {
        entered: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
    });
    let handle = start_listener_with_policy(
        config,
        Arc::new(IpcTypeRegistry::new()),
        Arc::new(DashMap::new()),
        CancellationToken::new(),
        policy,
    )
    .await
    .expect("listener");
    let mut first = UnixStream::connect(&socket).await.expect("connect");
    let rejected = response(&mut first).await;
    assert_eq!(rejected["error_code"], IPC_ACCESS_DENIED_CODE);
    assert!(rejected["error"]
        .as_str()
        .expect("reason")
        .contains("timed out"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.stats.connections_active() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capacity released");
    let mut second = UnixStream::connect(&socket).await.expect("second");
    let accepted = send(
        &mut second,
        Format::Json,
        MSG_TYPE_SUBSCRIBE,
        &json!({"correlation_id":"second","message_types":["Event"]}),
    )
    .await;
    assert_eq!(accepted["success"], true);
    handle.stop();
}

struct PanickingPolicy;
impl IpcSecurityPolicy for PanickingPolicy {
    fn admit(&self, _: IpcConnectionInfo) -> IpcAdmission<'_> {
        Box::pin(async { Ok(IpcIdentity::new("primitive")) })
    }
    fn authorize(
        &self,
        _: &IpcConnectionContext,
        operation: IpcOperation<'_>,
    ) -> Result<(), IpcAccessDenied> {
        assert!(
            !matches!(operation, IpcOperation::Request(_)),
            "application policy panic"
        );
        Ok(())
    }
}

#[tokio::test]
async fn policy_panic_closes_session_and_cleans_subscriptions_and_statistics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket = dir.path().join("ipc.sock");
    let handle = listener(&socket, Arc::new(PanickingPolicy)).await;
    let mut stream = UnixStream::connect(&socket).await.expect("connect");
    let subscribed = send(
        &mut stream,
        Format::Json,
        MSG_TYPE_SUBSCRIBE,
        &json!({"correlation_id":"subscribe","message_types":["Event"]}),
    )
    .await;
    assert_eq!(subscribed["success"], true);
    let context = handle
        .subscription_manager()
        .connection_context(1)
        .expect("session");
    let envelope = IpcEnvelope::new("missing", "Unknown", json!({}));
    write_frame(
        &mut stream,
        MSG_TYPE_REQUEST,
        Format::Json,
        &Format::Json.serialize(&envelope).expect("encode"),
    )
    .await
    .expect("write");
    assert!(tokio::time::timeout(
        Duration::from_secs(2),
        read_frame(&mut stream, MAX_FRAME_SIZE)
    )
    .await
    .expect("panic closes socket")
    .is_err());
    assert!(context.is_revoked());
    assert_eq!(handle.subscription_manager().connection_count(), 0);
    assert_eq!(handle.subscription_manager().total_subscriptions(), 0);
    assert_eq!(handle.stats.connections_active(), 0);
    assert_eq!(handle.stats.in_flight_requests(), 0);
    handle.stop();
}
