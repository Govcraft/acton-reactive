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

//! Tests for examples from docs/replies-and-context/page.md
//!
//! This module verifies that the code examples from the "Replies & Context"
//! documentation page compile and run correctly.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests accessing message data through context.
///
/// From: docs/replies-and-context/page.md - "Accessing the Message"
#[acton_test]
async fn test_accessing_message() -> anyhow::Result<()> {
    #[acton_actor]
    struct Processor {
        last_order_id: Option<String>,
    }

    #[acton_message]
    struct OrderRequest {
        id: String,
    }

    let last_id = Arc::new(std::sync::Mutex::new(String::new()));
    let last_id_clone = last_id.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Processor>();

    actor
        .mutate_on::<OrderRequest>(|actor, ctx| {
            // Get immutable reference
            let order: &OrderRequest = ctx.message();
            actor.model.last_order_id = Some(order.id.clone());
            Reply::ready()
        })
        .after_stop(move |actor| {
            if let Some(id) = &actor.model.last_order_id {
                last_id_clone.lock().unwrap().clone_from(id);
            }
            Reply::ready()
        });

    let handle = actor.start().await;

    handle
        .send(OrderRequest {
            id: "ORD-123".to_string(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(*last_id.lock().unwrap(), "ORD-123");

    Ok(())
}

/// Tests reply envelope for request-response.
///
/// From: docs/replies-and-context/page.md - "Reply Envelope"
///
/// Note: Request-reply in acton-reactive requires using `ctx.new_envelope()` to
/// maintain the proper reply chain. This test uses the three-actor pattern where
/// a trigger message initiates the request flow.
#[acton_test]
async fn test_reply_envelope() -> anyhow::Result<()> {
    #[acton_actor]
    struct Account {
        balance: f64,
    }

    #[acton_actor]
    struct Client {
        account_handle: Option<ActorHandle>,
        received_balance: Option<f64>,
    }

    #[acton_message]
    struct Trigger;

    #[acton_message]
    struct GetBalance;

    #[acton_message]
    struct BalanceResponse(f64);

    let received_balance = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let received_clone = received_balance.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create account (the responder)
    let mut account = runtime.new_actor::<Account>();
    account.model.balance = 1000.0;

    account.act_on::<GetBalance>(|actor, ctx| {
        let balance = actor.model.balance;
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            reply.send(BalanceResponse(balance)).await;
        })
    });

    let account_handle = account.start().await;

    // Create client (the requester)
    let mut client = runtime.new_actor::<Client>();
    client.model.account_handle = Some(account_handle);

    client
        .mutate_on::<Trigger>(|actor, ctx| {
            let target = actor.model.account_handle.clone().unwrap();
            // Use ctx.new_envelope() to maintain the reply chain
            let request_envelope = ctx.new_envelope(&target.reply_address());

            Reply::pending(async move {
                request_envelope.send(GetBalance).await;
            })
        })
        .mutate_on::<BalanceResponse>(move |actor, ctx| {
            actor.model.received_balance = Some(ctx.message().0);
            received_clone.store(ctx.message().0.to_bits(), Ordering::SeqCst);
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Trigger the request flow
    client_handle.send(Trigger).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    let balance = f64::from_bits(received_balance.load(Ordering::SeqCst));
    assert!((balance - 1000.0).abs() < f64::EPSILON);

    Ok(())
}

/// Tests fire-and-forget pattern (no reply).
///
/// From: docs/replies-and-context/page.md - "No Reply (Fire-and-Forget)"
#[acton_test]
async fn test_no_reply_pattern() -> anyhow::Result<()> {
    #[acton_actor]
    struct Logger {
        events: Vec<LogEvent>,
    }

    #[acton_message]
    struct LogEvent {
        message: String,
    }

    let event_count = Arc::new(AtomicU32::new(0));
    let count_clone = event_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut logger = runtime.new_actor::<Logger>();

    logger
        .mutate_on::<LogEvent>(|actor, ctx| {
            actor.model.events.push(ctx.message().clone());
            // No reply needed
            Reply::ready()
        })
        .after_stop(move |actor| {
            count_clone.store(
                u32::try_from(actor.model.events.len()).unwrap_or(u32::MAX),
                Ordering::SeqCst,
            );
            Reply::ready()
        });

    let handle = logger.start().await;

    handle
        .send(LogEvent {
            message: "Event 1".to_string(),
        })
        .await;
    handle
        .send(LogEvent {
            message: "Event 2".to_string(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert_eq!(event_count.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Tests multiple replies (streaming) pattern.
///
/// From: docs/replies-and-context/page.md - "Multiple Replies (Streaming)"
///
/// Note: Request-reply in acton-reactive requires using `ctx.new_envelope()` to
/// maintain the proper reply chain. This test uses the trigger pattern.
#[acton_test]
async fn test_streaming_replies() -> anyhow::Result<()> {
    #[acton_actor]
    struct DataSource {
        items: Vec<String>,
    }

    #[acton_actor]
    struct StreamClient {
        source_handle: Option<ActorHandle>,
        received_items: Vec<String>,
        complete: bool,
    }

    #[acton_message]
    struct StartStream;

    #[acton_message]
    struct StreamRequest;

    #[acton_message]
    struct StreamItem {
        data: String,
        index: usize,
    }

    #[acton_message]
    struct StreamComplete;

    let items_received = Arc::new(AtomicU32::new(0));
    let items_clone = items_received.clone();
    let completed = Arc::new(AtomicBool::new(false));
    let completed_clone = completed.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create data source (responder)
    let mut source = runtime.new_actor::<DataSource>();
    source.model.items = vec![
        "item1".to_string(),
        "item2".to_string(),
        "item3".to_string(),
    ];

    source.mutate_on::<StreamRequest>(|actor, ctx| {
        let items = actor.model.items.clone();
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            for (i, item) in items.iter().enumerate() {
                reply
                    .send(StreamItem {
                        data: item.clone(),
                        index: i,
                    })
                    .await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            reply.send(StreamComplete).await;
        })
    });

    let source_handle = source.start().await;

    // Create client (requester) that uses trigger pattern
    let mut client = runtime.new_actor::<StreamClient>();
    client.model.source_handle = Some(source_handle);

    client
        .mutate_on::<StartStream>(|actor, ctx| {
            let target = actor.model.source_handle.clone().unwrap();
            let request_envelope = ctx.new_envelope(&target.reply_address());

            Reply::pending(async move {
                request_envelope.send(StreamRequest).await;
            })
        })
        .mutate_on::<StreamItem>(move |actor, ctx| {
            actor.model.received_items.push(ctx.message().data.clone());
            items_clone.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        })
        .mutate_on::<StreamComplete>(move |actor, _ctx| {
            actor.model.complete = true;
            completed_clone.store(true, Ordering::SeqCst);
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Trigger stream via client
    client_handle.send(StartStream).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    runtime.shutdown_all().await?;

    assert_eq!(items_received.load(Ordering::SeqCst), 3);
    assert!(completed.load(Ordering::SeqCst));

    Ok(())
}

/// Tests deferred reply pattern.
///
/// From: docs/replies-and-context/page.md - "Deferred Reply"
///
/// Note: Request-reply in acton-reactive requires using `ctx.new_envelope()` to
/// maintain the proper reply chain. This test uses the trigger pattern.
#[acton_test]
async fn test_deferred_reply() -> anyhow::Result<()> {
    #[acton_actor]
    struct TaskProcessor {
        pending_replies: HashMap<u32, OutboundEnvelope>,
        next_task_id: u32,
    }

    #[acton_actor]
    struct TaskClient {
        processor_handle: Option<ActorHandle>,
        accepted_task: Option<u32>,
        task_result: Option<String>,
    }

    #[acton_message]
    struct SubmitTask;

    #[acton_message]
    struct LongRunningTask;

    #[acton_message]
    struct TaskAccepted {
        task_id: u32,
    }

    #[acton_message]
    struct CompleteTask {
        task_id: u32,
        result: String,
    }

    #[acton_message]
    struct TaskResult {
        result: String,
    }

    let task_accepted = Arc::new(AtomicU32::new(0));
    let task_accepted_clone = task_accepted.clone();
    let result_received = Arc::new(std::sync::Mutex::new(String::new()));
    let result_clone = result_received.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create processor (responder)
    let mut processor = runtime.new_actor::<TaskProcessor>();
    processor.model.next_task_id = 1;

    processor
        .mutate_on::<LongRunningTask>(|actor, ctx| {
            let task_id = actor.model.next_task_id;
            actor.model.next_task_id += 1;

            let reply = ctx.reply_envelope();

            // Store the reply channel for later
            actor.model.pending_replies.insert(task_id, reply.clone());

            // Acknowledge immediately
            Reply::pending(async move {
                reply.send(TaskAccepted { task_id }).await;
            })
        })
        .mutate_on::<CompleteTask>(|actor, ctx| {
            let task_id = ctx.message().task_id;

            if let Some(reply) = actor.model.pending_replies.remove(&task_id) {
                let result = ctx.message().result.clone();
                return Reply::pending(async move {
                    reply.send(TaskResult { result }).await;
                });
            }

            Reply::ready()
        });

    let processor_handle = processor.start().await;

    // Create client (requester) that uses trigger pattern
    let mut client = runtime.new_actor::<TaskClient>();
    client.model.processor_handle = Some(processor_handle.clone());

    client
        .mutate_on::<SubmitTask>(|actor, ctx| {
            let target = actor.model.processor_handle.clone().unwrap();
            let request_envelope = ctx.new_envelope(&target.reply_address());

            Reply::pending(async move {
                request_envelope.send(LongRunningTask).await;
            })
        })
        .mutate_on::<TaskAccepted>(move |actor, ctx| {
            actor.model.accepted_task = Some(ctx.message().task_id);
            task_accepted_clone.store(ctx.message().task_id, Ordering::SeqCst);
            Reply::ready()
        })
        .mutate_on::<TaskResult>(move |actor, ctx| {
            actor.model.task_result = Some(ctx.message().result.clone());
            result_clone
                .lock()
                .unwrap()
                .clone_from(&ctx.message().result);
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Trigger task submission via client
    client_handle.send(SubmitTask).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Task should be accepted
    let task_id = task_accepted.load(Ordering::SeqCst);
    assert!(task_id > 0);

    // Complete the task
    processor_handle
        .send(CompleteTask {
            task_id,
            result: "Done!".to_string(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert_eq!(*result_received.lock().unwrap(), "Done!");

    Ok(())
}

/// Tests request-reply with timeout.
///
/// From: docs/replies-and-context/page.md - "Request-Reply with Timeout"
///
/// Note: Request-reply in acton-reactive requires using `ctx.new_envelope()` to
/// maintain the proper reply chain. This test uses the trigger pattern.
#[acton_test]
async fn test_request_reply_with_timeout() -> anyhow::Result<()> {
    #[acton_actor]
    struct SlowService {
        delay_ms: u64,
    }

    #[acton_actor]
    struct TimeoutClient {
        service_handle: Option<ActorHandle>,
        success: bool,
        timeout: bool,
    }

    #[acton_message]
    struct SendQuery;

    #[acton_message]
    struct Query;

    #[acton_message]
    struct QuerySuccess(String);

    #[acton_message]
    struct QueryTimeout;

    let success_received = Arc::new(AtomicBool::new(false));
    let success_clone = success_received.clone();
    let timeout_received = Arc::new(AtomicBool::new(false));
    let timeout_clone = timeout_received.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create fast service (responder - will succeed)
    let mut fast_service = runtime.new_actor::<SlowService>();
    fast_service.model.delay_ms = 10;

    fast_service.act_on::<Query>(|actor, ctx| {
        let delay = actor.model.delay_ms;
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            match tokio::time::timeout(Duration::from_millis(50), async {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                "Result".to_string()
            })
            .await
            {
                Ok(result) => reply.send(QuerySuccess(result)).await,
                Err(_) => reply.send(QueryTimeout).await,
            }
        })
    });

    let fast_handle = fast_service.start().await;

    // Create client (requester) that uses trigger pattern
    let mut client = runtime.new_actor::<TimeoutClient>();
    client.model.service_handle = Some(fast_handle);

    client
        .mutate_on::<SendQuery>(|actor, ctx| {
            let target = actor.model.service_handle.clone().unwrap();
            let request_envelope = ctx.new_envelope(&target.reply_address());

            Reply::pending(async move {
                request_envelope.send(Query).await;
            })
        })
        .mutate_on::<QuerySuccess>(move |actor, _ctx| {
            actor.model.success = true;
            success_clone.store(true, Ordering::SeqCst);
            Reply::ready()
        })
        .mutate_on::<QueryTimeout>(move |actor, _ctx| {
            actor.model.timeout = true;
            timeout_clone.store(true, Ordering::SeqCst);
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Trigger query via client
    client_handle.send(SendQuery).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    // Fast service should succeed
    assert!(success_received.load(Ordering::SeqCst));

    Ok(())
}

/// Tests acknowledgment pattern.
///
/// From: docs/replies-and-context/page.md - "Acknowledgment Pattern"
///
/// Note: Request-reply in acton-reactive requires using `ctx.new_envelope()` to
/// maintain the proper reply chain. This test uses the trigger pattern.
#[acton_test]
async fn test_acknowledgment_pattern() -> anyhow::Result<()> {
    #[acton_actor]
    struct Processor {
        processed: bool,
    }

    #[acton_actor]
    struct AckClient {
        processor_handle: Option<ActorHandle>,
        ack_received: bool,
    }

    #[acton_message]
    struct SendMessage;

    #[acton_message]
    struct ImportantMessage {
        data: String,
    }

    #[acton_message]
    struct Ack;

    let ack_received = Arc::new(AtomicBool::new(false));
    let ack_clone = ack_received.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create processor (responder)
    let mut processor = runtime.new_actor::<Processor>();

    processor.mutate_on::<ImportantMessage>(|actor, ctx| {
        let reply = ctx.reply_envelope();

        // Process the message
        actor.model.processed = true;

        // Acknowledge
        Reply::pending(async move {
            reply.send(Ack).await;
        })
    });

    let processor_handle = processor.start().await;

    // Create client (requester) that uses trigger pattern
    let mut client = runtime.new_actor::<AckClient>();
    client.model.processor_handle = Some(processor_handle);

    client
        .mutate_on::<SendMessage>(|actor, ctx| {
            let target = actor.model.processor_handle.clone().unwrap();
            let request_envelope = ctx.new_envelope(&target.reply_address());

            Reply::pending(async move {
                request_envelope
                    .send(ImportantMessage {
                        data: "test".to_string(),
                    })
                    .await;
            })
        })
        .mutate_on::<Ack>(move |actor, _ctx| {
            actor.model.ack_received = true;
            ack_clone.store(true, Ordering::SeqCst);
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Trigger message via client
    client_handle.send(SendMessage).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert!(ack_received.load(Ordering::SeqCst));

    Ok(())
}

/// Tests convenience reply method.
///
/// From: docs/replies-and-context/page.md - "Convenience Reply"
///
/// Note: Request-reply in acton-reactive requires using `ctx.new_envelope()` to
/// maintain the proper reply chain. This test uses the trigger pattern.
#[acton_test]
async fn test_convenience_reply() -> anyhow::Result<()> {
    #[acton_actor]
    struct Service {
        value: u32,
    }

    #[acton_actor]
    struct QueryClient {
        service_handle: Option<ActorHandle>,
    }

    #[acton_message]
    struct SendQuery;

    #[acton_message]
    struct Query;

    #[acton_message]
    struct QueryResponse {
        value: u32,
    }

    let received_value = Arc::new(AtomicU32::new(0));
    let received_clone = received_value.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create service (responder)
    let mut service = runtime.new_actor::<Service>();
    service.model.value = 42;

    service.mutate_on::<Query>(|actor, ctx| {
        let value = actor.model.value;
        let reply = ctx.reply_envelope();
        // Send reply using reply envelope
        Reply::pending(async move {
            reply.send(QueryResponse { value }).await;
        })
    });

    let service_handle = service.start().await;

    // Create client (requester) that uses trigger pattern
    let mut client = runtime.new_actor::<QueryClient>();
    client.model.service_handle = Some(service_handle);

    client
        .mutate_on::<SendQuery>(|actor, ctx| {
            let target = actor.model.service_handle.clone().unwrap();
            let request_envelope = ctx.new_envelope(&target.reply_address());

            Reply::pending(async move {
                request_envelope.send(Query).await;
            })
        })
        .mutate_on::<QueryResponse>(move |_actor, ctx| {
            received_clone.store(ctx.message().value, Ordering::SeqCst);
            Reply::ready()
        });

    let client_handle = client.start().await;

    // Trigger query via client
    client_handle.send(SendQuery).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert_eq!(received_value.load(Ordering::SeqCst), 42);

    Ok(())
}
