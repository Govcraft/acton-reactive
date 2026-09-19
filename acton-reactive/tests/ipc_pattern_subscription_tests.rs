//! Pattern subscriptions across the public client and IPC framing boundary.
#![cfg(feature = "ipc")]

use std::sync::Arc;
use std::time::Duration;

use acton_reactive::ipc::protocol::{
    read_frame, write_pattern_subscription_response_with_format,
    write_subscribe_patterns_with_format, write_unsubscribe_patterns_with_format, Format,
    MAX_FRAME_SIZE, MSG_TYPE_RESPONSE, MSG_TYPE_SUBSCRIBE_PATTERNS, MSG_TYPE_UNSUBSCRIBE_PATTERNS,
};
use acton_reactive::ipc::{
    start_listener, IpcClient, IpcClientConfig, IpcConfig, IpcPatternSubscribeRequest,
    IpcPatternSubscriptionResponse, IpcPatternUnsubscribeRequest, IpcPushNotification,
    IpcTypeRegistry, SocketConfig,
};
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

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

#[tokio::test]
async fn pattern_frames_round_trip_in_both_formats() {
    for format in formats() {
        let (mut writer, mut reader) = tokio::io::duplex(4096);
        let request = IpcPatternSubscribeRequest::with_correlation_id(
            "pattern_request",
            vec!["Order*".into(), "*".into()],
        );
        write_subscribe_patterns_with_format(&mut writer, &request, format)
            .await
            .unwrap();
        let (tag, received_format, bytes) = read_frame(&mut reader, MAX_FRAME_SIZE).await.unwrap();
        assert_eq!(tag, MSG_TYPE_SUBSCRIBE_PATTERNS);
        assert_eq!(received_format, format);
        let decoded: IpcPatternSubscribeRequest = format.deserialize(&bytes).unwrap();
        assert_eq!(decoded.correlation_id, request.correlation_id);
        assert_eq!(decoded.patterns, request.patterns);

        let request =
            IpcPatternUnsubscribeRequest::with_correlation_id("clear_patterns", Vec::new());
        write_unsubscribe_patterns_with_format(&mut writer, &request, format)
            .await
            .unwrap();
        let (tag, _, bytes) = read_frame(&mut reader, MAX_FRAME_SIZE).await.unwrap();
        assert_eq!(tag, MSG_TYPE_UNSUBSCRIBE_PATTERNS);
        let decoded: IpcPatternUnsubscribeRequest = format.deserialize(&bytes).unwrap();
        assert_eq!(decoded.correlation_id, "clear_patterns");
        assert!(decoded.patterns.is_empty());

        let response = IpcPatternSubscriptionResponse::error("pattern_request", "invalid pattern");
        write_pattern_subscription_response_with_format(&mut writer, &response, format)
            .await
            .unwrap();
        let (tag, _, bytes) = read_frame(&mut reader, MAX_FRAME_SIZE).await.unwrap();
        assert_eq!(tag, MSG_TYPE_RESPONSE);
        let decoded: IpcPatternSubscriptionResponse = format.deserialize(&bytes).unwrap();
        assert_eq!(decoded.correlation_id, "pattern_request");
        assert!(!decoded.success);
        assert_eq!(decoded.error.as_deref(), Some("invalid pattern"));
    }
}

#[tokio::test]
async fn client_patterns_deliver_once_and_mutate_atomically_in_both_formats() {
    for format in formats() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("patterns.sock");
        let cancel = CancellationToken::new();
        let handle = start_listener(
            IpcConfig {
                socket: SocketConfig {
                    path: Some(socket.clone()),
                    ..SocketConfig::default()
                },
                ..IpcConfig::default()
            },
            Arc::new(IpcTypeRegistry::new()),
            Arc::new(DashMap::new()),
            cancel.clone(),
        )
        .await
        .unwrap();
        let client = IpcClient::connect_with_config(
            &socket,
            IpcClientConfig {
                format,
                ..IpcClientConfig::default()
            },
        )
        .await
        .unwrap();
        let mut pushes = client.take_push_receiver().unwrap();
        assert!(
            client
                .subscribe(vec!["OrderCreated".into()])
                .await
                .unwrap()
                .success
        );
        let response = client
            .subscribe_patterns(vec!["Order*".into(), "*".into()])
            .await
            .unwrap();
        assert!(response.success);
        assert!(!response.correlation_id.is_empty());
        assert_eq!(response.subscribed_patterns, vec!["*", "Order*"]);
        let manager = handle.subscription_manager();
        let notification = IpcPushNotification::new("OrderCreated", None, serde_json::json!({}));
        manager.forward_to_subscribers(&notification);
        let received = tokio::time::timeout(Duration::from_secs(5), pushes.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received.notification_id, notification.notification_id);
        assert_eq!(
            manager.stats().push_notifications_sent(),
            1,
            "overlap must deduplicate"
        );

        // Names first encountered after subscription are matched immediately.
        manager.forward_to_subscribers(&IpcPushNotification::new(
            "FutureEvent",
            None,
            serde_json::json!({}),
        ));
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), pushes.recv())
                .await
                .unwrap()
                .unwrap()
                .message_type,
            "FutureEvent"
        );

        verify_subscription_mutations(&client, manager, &notification, &mut pushes).await;
        client.disconnect().await.unwrap();
        cancel.cancel();
    }
}

async fn verify_subscription_mutations(
    client: &IpcClient,
    manager: &acton_reactive::ipc::SubscriptionManager,
    notification: &IpcPushNotification,
    pushes: &mut tokio::sync::mpsc::Receiver<IpcPushNotification>,
) {
    let rejected = client
        .subscribe_patterns(vec!["New*".into(), "invalid".into()])
        .await
        .unwrap();
    assert!(!rejected.success);
    assert!(rejected.error.is_some());
    assert!(!rejected.correlation_id.is_empty());
    assert_eq!(manager.get_pattern_subscriptions(1), vec!["*", "Order*"]);
    for invalid in [vec!["x".repeat(256) + "*"], vec!["A*".into(); 129]] {
        assert!(!client.subscribe_patterns(invalid).await.unwrap().success);
        assert_eq!(manager.get_pattern_subscriptions(1), vec!["*", "Order*"]);
    }

    assert!(
        client
            .unsubscribe_patterns(vec!["*".into()])
            .await
            .unwrap()
            .success
    );
    let sent = manager.stats().push_notifications_sent();
    manager.forward_to_subscribers(&IpcPushNotification::new(
        "FutureEvent",
        None,
        serde_json::json!({}),
    ));
    assert_eq!(manager.stats().push_notifications_sent(), sent);
    // Removing the exact selector leaves the overlapping pattern active.
    assert!(
        client
            .unsubscribe(vec!["OrderCreated".into()])
            .await
            .unwrap()
            .success
    );
    manager.forward_to_subscribers(notification);
    assert!(tokio::time::timeout(Duration::from_secs(5), pushes.recv())
        .await
        .unwrap()
        .is_some());
    assert!(manager.get_subscriptions(1).is_empty());
    assert!(manager.has_subscriptions(1));

    assert!(
        client
            .subscribe(vec!["OrderCreated".into()])
            .await
            .unwrap()
            .success
    );
    assert!(
        client
            .unsubscribe_patterns(Vec::new())
            .await
            .unwrap()
            .success
    );
    assert!(manager.get_pattern_subscriptions(1).is_empty());
    assert_eq!(manager.get_subscriptions(1), vec!["OrderCreated"]);
    assert!(
        client
            .subscribe_patterns(vec!["*".into()])
            .await
            .unwrap()
            .success
    );
    assert!(client.unsubscribe(Vec::new()).await.unwrap().success);
    assert!(!manager.has_subscriptions(1));
    let sent = manager.stats().push_notifications_sent();
    manager.forward_to_subscribers(notification);
    assert_eq!(manager.stats().push_notifications_sent(), sent);
}

#[tokio::test]
async fn pattern_rate_limits_are_correlated_counted_and_leave_state_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("limited.sock");
    let cancel = CancellationToken::new();
    let handle = start_listener(
        IpcConfig {
            socket: SocketConfig {
                path: Some(socket.clone()),
                ..SocketConfig::default()
            },
            rate_limit: acton_reactive::ipc::RateLimitConfig {
                enabled: true,
                requests_per_second: 0,
                burst_size: 1,
            },
            ..IpcConfig::default()
        },
        Arc::new(IpcTypeRegistry::new()),
        Arc::new(DashMap::new()),
        cancel.clone(),
    )
    .await
    .unwrap();
    let client = IpcClient::connect(&socket).await.unwrap();
    assert!(
        client
            .subscribe_patterns(vec!["Order*".into()])
            .await
            .unwrap()
            .success
    );
    let rejected = client.unsubscribe_patterns(Vec::new()).await.unwrap();
    assert!(!rejected.success);
    assert!(rejected.error.unwrap().contains("rate limit"));
    assert!(!rejected.correlation_id.is_empty());
    assert_eq!(handle.stats.rate_limited(), 1);
    assert_eq!(
        handle.subscription_manager().get_pattern_subscriptions(1),
        vec!["Order*"]
    );
    // Existing exact unsubscribe-all still works after exhausting pattern tokens.
    assert!(client.unsubscribe(Vec::new()).await.unwrap().success);
    assert!(!handle.subscription_manager().has_subscriptions(1));
    client.disconnect().await.unwrap();
    cancel.cancel();
}

#[tokio::test]
async fn pattern_only_connection_outlives_an_idle_unsubscribed_connection() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("idle.sock");
    let cancel = CancellationToken::new();
    let handle = start_listener(
        IpcConfig {
            socket: SocketConfig {
                path: Some(socket.clone()),
                ..SocketConfig::default()
            },
            timeouts: acton_reactive::ipc::IpcTimeoutsConfig {
                read: 1000,
                subscription_read: 0,
                ..acton_reactive::ipc::IpcTimeoutsConfig::default()
            },
            ..IpcConfig::default()
        },
        Arc::new(IpcTypeRegistry::new()),
        Arc::new(DashMap::new()),
        cancel.clone(),
    )
    .await
    .unwrap();
    let subscriber = IpcClient::connect(&socket).await.unwrap();
    assert!(
        subscriber
            .subscribe_patterns(vec!["*".into()])
            .await
            .unwrap()
            .success
    );
    let mut subscribed_pushes = subscriber.take_push_receiver().unwrap();
    let idle = IpcClient::connect(&socket).await.unwrap();
    assert!(idle.unsubscribe(Vec::new()).await.unwrap().success);
    let mut idle_pushes = idle.take_push_receiver().unwrap();
    // Observe the actual non-subscriber timeout, rather than sleeping and guessing.
    assert!(
        tokio::time::timeout(Duration::from_secs(5), idle_pushes.recv())
            .await
            .unwrap()
            .is_none()
    );
    handle
        .subscription_manager()
        .forward_to_subscribers(&IpcPushNotification::new(
            "StillConnected",
            None,
            serde_json::json!({}),
        ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), subscribed_pushes.recv())
            .await
            .unwrap()
            .unwrap()
            .message_type,
        "StillConnected"
    );
    subscriber.disconnect().await.unwrap();
    cancel.cancel();
}

#[acton_reactive::prelude::acton_message(ipc)]
struct LateOrder {
    number: u32,
}

#[acton_reactive::prelude::acton_message]
struct InternalOnly;

#[tokio::test]
async fn broker_routes_a_type_registered_after_the_pattern_subscription() {
    use acton_reactive::prelude::*;

    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("broker.sock");
    let mut runtime = ActonApp::launch_async().await;
    let listener = runtime
        .start_ipc_listener_with_config(IpcConfig {
            socket: SocketConfig {
                path: Some(socket.clone()),
                ..SocketConfig::default()
            },
            ..IpcConfig::default()
        })
        .await
        .unwrap();
    let client = IpcClient::connect(&socket).await.unwrap();
    let mut pushes = client.take_push_receiver().unwrap();
    assert!(
        client
            .subscribe_patterns(vec!["Order*".into()])
            .await
            .unwrap()
            .success
    );
    runtime
        .ipc_registry()
        .register::<LateOrder>("OrderRegisteredLater");
    let broker = runtime.broker();
    broker.broadcast(LateOrder { number: 42 }).await;
    broker.ask(FlushBroadcasts).await.unwrap();
    let notification = tokio::time::timeout(Duration::from_secs(5), pushes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notification.message_type, "OrderRegisteredLater");
    assert_eq!(notification.payload, serde_json::json!({ "number": 42 }));
    assert!(
        client
            .subscribe_patterns(vec!["*".into()])
            .await
            .unwrap()
            .success
    );
    broker.broadcast(InternalOnly).await;
    broker.ask(FlushBroadcasts).await.unwrap();
    assert_eq!(
        listener
            .subscription_manager()
            .stats()
            .push_notifications_sent(),
        1,
        "catch-all patterns must not expose internal unregistered messages"
    );
    client.disconnect().await.unwrap();
    runtime.shutdown_all().await.unwrap();
}
