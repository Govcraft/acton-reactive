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

//! Tests for examples from docs/building-apps/request-response/page.md
//!
//! This module verifies that the code examples from the "Replies & Context"
//! documentation page compile and run correctly.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests accessing message data through context.
///
/// From: docs/building-apps/request-response/page.md - "Accessing the Message"
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

    runtime.shutdown_all().await?;

    assert_eq!(*last_id.lock().unwrap(), "ORD-123");

    Ok(())
}

/// Tests reply envelope for request-response.
///
/// From: docs/building-apps/request-response/page.md - "Reply Envelope"
///
/// The reply envelope is the handler's side of a reply. `ask` is the caller's side:
/// it stamps a private reply address on the request, so `ctx.reply_envelope()` sends
/// the answer straight back to the awaiting caller.
#[acton_test]
async fn test_reply_envelope() -> anyhow::Result<()> {
    #[acton_actor]
    struct Account {
        balance: f64,
    }

    #[acton_message]
    struct GetBalance;

    #[acton_message]
    struct BalanceResponse(f64);

    impl Request for GetBalance {
        type Response = BalanceResponse;
    }

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

    let response: BalanceResponse = account_handle.ask(GetBalance).await?;
    assert!((response.0 - 1000.0).abs() < f64::EPSILON);

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests fire-and-forget pattern (no reply).
///
/// From: docs/building-apps/request-response/page.md - "No Reply (Fire-and-Forget)"
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

    runtime.shutdown_all().await?;

    assert_eq!(event_count.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Tests multiple replies (streaming) pattern.
///
/// From: docs/building-apps/request-response/page.md - "Multiple Replies (Streaming)"
///
/// A stream is the one shape `ask` cannot express on its own, because `ask` waits for
/// exactly one reply and a stream sends many. The receiving actor therefore stays, and
/// the way to wait for a stream is to ask *it* whether the stream has finished. Note
/// that it holds the reply envelope when the answer is not ready yet: that is what
/// makes the result independent of whether the request arrives before or after the
/// last item, rather than merely likely to.
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
        /// A caller waiting to be told the stream is finished.
        waiting: Option<OutboundEnvelope>,
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

    /// Asks the client whether the stream has finished.
    #[acton_message]
    struct AwaitStream;

    /// The client's answer: the stream is done, and this is how much of it arrived.
    #[acton_message]
    struct StreamFinished(u32);

    impl Request for AwaitStream {
        type Response = StreamFinished;
    }

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
        .mutate_on::<StreamItem>(|actor, ctx| {
            actor.model.received_items.push(ctx.message().data.clone());
            Reply::ready()
        })
        .mutate_on::<StreamComplete>(|actor, _ctx| {
            actor.model.complete = true;

            // If someone already asked, answer them now.
            let count = u32::try_from(actor.model.received_items.len()).unwrap();
            if let Some(reply) = actor.model.waiting.take() {
                Reply::pending(async move {
                    reply.send(StreamFinished(count)).await;
                })
            } else {
                Reply::ready()
            }
        })
        .mutate_on::<AwaitStream>(|actor, ctx| {
            let reply = ctx.reply_envelope();
            if actor.model.complete {
                let count = u32::try_from(actor.model.received_items.len()).unwrap();
                Reply::pending(async move {
                    reply.send(StreamFinished(count)).await;
                })
            } else {
                // Not finished yet - keep the envelope and answer from StreamComplete.
                actor.model.waiting = Some(reply);
                Reply::ready()
            }
        });

    let client_handle = client.start().await;

    // Trigger stream via client
    client_handle.send(StartStream).await;

    let finished: StreamFinished = client_handle.ask(AwaitStream).await?;
    assert_eq!(finished.0, 3);

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests deferred reply pattern.
///
/// From: docs/building-apps/request-response/page.md - "Deferred Reply"
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
        /// A caller waiting to hear that the task was accepted.
        awaiting_acceptance: Option<OutboundEnvelope>,
    }

    #[acton_message]
    struct SubmitTask;

    impl Request for SubmitTask {
        type Response = TaskAccepted;
    }

    /// Asks the client what result it has recorded.
    #[acton_message]
    struct GetResult;

    #[acton_message]
    struct ResultReport {
        result: Option<String>,
    }

    impl Request for GetResult {
        type Response = ResultReport;
    }

    /// Acknowledges a [`CompleteTask`], reporting whether a pending task matched.
    #[acton_message]
    struct Completed {
        delivered: bool,
    }

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

    impl Request for CompleteTask {
        type Response = Completed;
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
            // The acknowledgement goes back to whoever sent `CompleteTask`; the stored
            // envelope goes back to the client that originally submitted the task.
            let ack = ctx.reply_envelope();

            if let Some(reply) = actor.model.pending_replies.remove(&task_id) {
                let result = ctx.message().result.clone();
                return Reply::pending(async move {
                    reply.send(TaskResult { result }).await;
                    // Acknowledged only after the result is queued for the client, so a
                    // caller holding this acknowledgement knows the client will see the
                    // result before anything the caller sends next.
                    ack.send(Completed { delivered: true }).await;
                });
            }

            Reply::pending(async move {
                ack.send(Completed { delivered: false }).await;
            })
        });

    let processor_handle = processor.start().await;

    // Create client (requester) that uses trigger pattern
    let mut client = runtime.new_actor::<TaskClient>();
    client.model.processor_handle = Some(processor_handle.clone());

    client
        .mutate_on::<SubmitTask>(|actor, ctx| {
            let target = actor.model.processor_handle.clone().unwrap();
            let request_envelope = ctx.new_envelope(&target.reply_address());

            // Hold the caller's reply envelope. The acceptance the caller wants does not
            // exist yet — it arrives later, as a `TaskAccepted` from the processor — so
            // the answer is deferred until that lands rather than guessed at now.
            actor.model.awaiting_acceptance = Some(ctx.reply_envelope());

            Reply::pending(async move {
                request_envelope.send(LongRunningTask).await;
            })
        })
        .mutate_on::<TaskAccepted>(move |actor, ctx| {
            let task_id = ctx.message().task_id;
            actor.model.accepted_task = Some(task_id);
            task_accepted_clone.store(task_id, Ordering::SeqCst);

            if let Some(reply) = actor.model.awaiting_acceptance.take() {
                return Reply::pending(async move {
                    reply.send(TaskAccepted { task_id }).await;
                });
            }

            Reply::ready()
        })
        .mutate_on::<GetResult>(|actor, ctx| {
            let reply = ctx.reply_envelope();
            let result = actor.model.task_result.clone();
            Reply::pending(async move {
                reply.send(ResultReport { result }).await;
            })
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

    // Trigger task submission via client. The client answers only once the processor's
    // `TaskAccepted` has actually reached it, so the id below is the real one rather
    // than whatever the atomic happened to hold.
    let accepted: TaskAccepted = client_handle.ask(SubmitTask).await?;
    let task_id = accepted.task_id;
    assert!(task_id > 0, "the processor must have assigned a task id");
    assert_eq!(
        task_id,
        task_accepted.load(Ordering::SeqCst),
        "the client must have recorded the same id it reported"
    );

    // Complete the task. The acknowledgement is sent after the result is queued for the
    // client, so this also establishes that the client's `TaskResult` is already in its
    // inbox — ahead of the `GetResult` below.
    let completed: Completed = processor_handle
        .ask(CompleteTask {
            task_id,
            result: "Done!".to_string(),
        })
        .await?;
    assert!(
        completed.delivered,
        "the task id must have matched a pending reply"
    );

    // FIFO puts this behind the `TaskResult` the processor just queued.
    let report: ResultReport = client_handle.ask(GetResult).await?;
    assert_eq!(report.result.as_deref(), Some("Done!"));

    runtime.shutdown_all().await?;

    assert_eq!(*result_received.lock().unwrap(), "Done!");

    Ok(())
}

/// Tests bounding a request with a deadline.
///
/// From: docs/building-apps/request-response/page.md - "Request-Reply with Timeout"
///
/// Every `ask` is already bounded by [`DEFAULT_ASK_TIMEOUT`]; `ask_with_timeout` sets a
/// different bound. The deadline is a backstop rather than the usual way a request
/// ends - a request whose reply address is dropped fails immediately with a specific
/// error instead of waiting for the clock.
///
/// The sleeps below are the slow work being modelled, not a way to synchronise the
/// test. They are what the deadline is measured against.
#[acton_test]
async fn test_request_reply_with_timeout() -> anyhow::Result<()> {
    #[acton_actor]
    struct SlowService {
        delay_ms: u64,
    }

    #[acton_message]
    struct Query;

    #[acton_message]
    struct QuerySuccess(String);

    impl Request for Query {
        type Response = QuerySuccess;
    }

    let mut runtime = ActonApp::launch_async().await;

    // A service that answers well inside the deadline.
    let mut fast_service = runtime.new_actor::<SlowService>();
    fast_service.model.delay_ms = 10;

    fast_service.act_on::<Query>(|actor, ctx| {
        let delay = actor.model.delay_ms;
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            reply.send(QuerySuccess("Result".to_string())).await;
        })
    });

    let fast_handle = fast_service.start().await;

    let result: QuerySuccess = fast_handle
        .ask_with_timeout(Query, Duration::from_millis(500))
        .await?;
    assert_eq!(result.0, "Result");

    // A service that cannot answer in time. The caller gets a `TimedOut` telling it how
    // long it waited, rather than being left hanging.
    let mut slow_service = runtime.new_actor::<SlowService>();
    slow_service.model.delay_ms = 500;

    slow_service.act_on::<Query>(|actor, ctx| {
        let delay = actor.model.delay_ms;
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            reply.send(QuerySuccess("Too late".to_string())).await;
        })
    });

    let slow_handle = slow_service.start().await;

    let outcome = slow_handle
        .ask_with_timeout(Query, Duration::from_millis(20))
        .await;
    assert!(
        matches!(outcome, Err(AskError::TimedOut { .. })),
        "expected the deadline to be reported, got {outcome:?}"
    );

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests acknowledgment pattern.
///
/// From: docs/building-apps/request-response/page.md - "Acknowledgment Pattern"
///
/// An acknowledgment is a reply that carries no data: the point is that it arrived.
/// `ask` is exactly that - it resolves once the handler has answered, so receiving the
/// `Ack` is proof the message was processed.
#[acton_test]
async fn test_acknowledgment_pattern() -> anyhow::Result<()> {
    #[acton_actor]
    struct Processor {
        processed: bool,
    }

    #[acton_message]
    struct ImportantMessage {
        data: String,
    }

    #[acton_message]
    struct Ack;

    impl Request for ImportantMessage {
        type Response = Ack;
    }

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

    let _: Ack = processor_handle
        .ask(ImportantMessage {
            data: "test".to_string(),
        })
        .await?;

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests convenience reply method.
///
/// From: docs/building-apps/request-response/page.md - "Convenience Reply"
#[acton_test]
async fn test_convenience_reply() -> anyhow::Result<()> {
    #[acton_actor]
    struct Service {
        value: u32,
    }

    #[acton_message]
    struct Query;

    #[acton_message]
    struct QueryResponse {
        value: u32,
    }

    impl Request for Query {
        type Response = QueryResponse;
    }

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

    let response: QueryResponse = service_handle.ask(Query).await?;
    assert_eq!(response.value, 42);

    runtime.shutdown_all().await?;

    Ok(())
}
