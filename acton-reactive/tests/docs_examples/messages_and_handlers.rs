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

//! Tests for examples from docs/core-concepts/messages-and-handlers/page.md
//!
//! This module verifies that the code examples from the "Messages & Handlers"
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests defining various message types.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Defining Messages"
#[acton_test]
async fn test_message_types() -> anyhow::Result<()> {
    // Simple message with no data
    #[acton_message]
    struct Ping;

    // Message with data
    #[acton_message]
    struct AddItem {
        name: String,
        quantity: u32,
    }

    // Tuple struct message
    #[acton_message]
    struct Increment(u32);

    // Message with complex data
    #[derive(Clone, Debug, Default)]
    struct OrderItem {
        name: String,
        price: f64,
    }

    #[derive(Clone, Debug, Default)]
    struct Customer {
        id: String,
    }

    #[acton_message]
    struct ProcessOrder {
        order_id: String,
        items: Vec<OrderItem>,
        customer: Customer,
    }

    #[allow(clippy::struct_excessive_bools)]
    #[acton_actor]
    struct TestActor {
        received_ping: bool,
        received_item: bool,
        received_increment: bool,
        received_order: bool,
    }

    let all_received = Arc::new(AtomicBool::new(false));
    let all_received_clone = all_received.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<TestActor>();

    actor
        .mutate_on::<Ping>(|actor, _ctx| {
            actor.model.received_ping = true;
            Reply::ready()
        })
        .mutate_on::<AddItem>(|actor, _ctx| {
            actor.model.received_item = true;
            Reply::ready()
        })
        .mutate_on::<Increment>(|actor, _ctx| {
            actor.model.received_increment = true;
            Reply::ready()
        })
        .mutate_on::<ProcessOrder>(|actor, _ctx| {
            actor.model.received_order = true;
            Reply::ready()
        })
        .after_stop(move |actor| {
            all_received_clone.store(
                actor.model.received_ping
                    && actor.model.received_item
                    && actor.model.received_increment
                    && actor.model.received_order,
                Ordering::SeqCst,
            );
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(Ping).await;
    handle
        .send(AddItem {
            name: "test".to_string(),
            quantity: 1,
        })
        .await;
    handle.send(Increment(5)).await;
    handle
        .send(ProcessOrder {
            order_id: "123".to_string(),
            items: vec![OrderItem::default()],
            customer: Customer::default(),
        })
        .await;

    runtime.shutdown_all().await?;

    assert!(all_received.load(Ordering::SeqCst));

    Ok(())
}

/// Tests handler anatomy - actor and ctx parameters.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Handler Anatomy"
#[acton_test]
async fn test_handler_anatomy() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyState {
        count: u32,
    }

    #[acton_message]
    struct MyMessage {
        value: u32,
    }

    let final_count = Arc::new(AtomicU32::new(0));
    let final_count_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<MyState>();

    actor
        .mutate_on::<MyMessage>(|actor, ctx| {
            // `actor` - the ManagedActor with state access
            // `ctx` - MessageContext with the message and reply tools

            // Access the actor's state (use black_box to prevent optimization)
            std::hint::black_box(actor.model.count);

            // Access the incoming message
            let message = ctx.message();
            actor.model.count += message.value;

            // Return a Reply
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(MyMessage { value: 10 }).await;
    handle.send(MyMessage { value: 20 }).await;

    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 30);

    Ok(())
}

/// Tests the actor parameter - accessing state, handle, id, and broker.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "The `actor` Parameter"
#[acton_test]
async fn test_actor_parameter() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyState {
        count: u32,
    }

    #[acton_message]
    struct TestMessage;

    #[acton_message]
    struct SelfMessage;

    let self_message_received = Arc::new(AtomicBool::new(false));
    let self_message_clone = self_message_received.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<MyState>();

    actor
        .mutate_on::<TestMessage>(|actor, _ctx| {
            // Access state
            actor.model.count += 1;

            // Get actor's handle (for self-messaging)
            let self_handle = actor.handle().clone();

            // Get actor's ID
            let _id = actor.id();

            // Get the broker (for pub/sub)
            let _broker = actor.broker();

            Reply::pending(async move {
                // Send message to self
                self_handle.send(SelfMessage).await;
            })
        })
        .mutate_on::<SelfMessage>(move |actor, _ctx| {
            actor.model.count += 100;
            Reply::ready()
        })
        .after_stop(move |actor| {
            self_message_clone.store(actor.model.count > 100, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;
    handle.send(TestMessage).await;

    runtime.shutdown_all().await?;

    assert!(self_message_received.load(Ordering::SeqCst));

    Ok(())
}

/// Tests Reply types - `ready()` and `pending()`.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Reply Types"
#[acton_test]
async fn test_reply_types() -> anyhow::Result<()> {
    #[acton_actor]
    struct MyState {
        sync_count: u32,
        async_count: u32,
    }

    #[acton_message]
    struct SimpleMessage;

    #[acton_message]
    struct AsyncMessage {
        data: String,
    }

    let counts_verified = Arc::new(AtomicBool::new(false));
    let counts_clone = counts_verified.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<MyState>();

    actor
        // Immediate Completion
        .mutate_on::<SimpleMessage>(|actor, _ctx| {
            actor.model.sync_count += 1;
            Reply::ready() // Done immediately
        })
        // Async Completion
        .mutate_on::<AsyncMessage>(|actor, ctx| {
            let data = ctx.message().data.clone();
            actor.model.async_count += 1;

            Reply::pending(async move {
                // Async work happens here
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _ = data; // Use the data
            })
        })
        .after_stop(move |actor| {
            counts_clone.store(
                actor.model.sync_count == 2 && actor.model.async_count == 2,
                Ordering::SeqCst,
            );
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(SimpleMessage).await;
    handle.send(SimpleMessage).await;
    handle
        .send(AsyncMessage {
            data: "test1".to_string(),
        })
        .await;
    handle
        .send(AsyncMessage {
            data: "test2".to_string(),
        })
        .await;

    runtime.shutdown_all().await?;

    assert!(counts_verified.load(Ordering::SeqCst));

    Ok(())
}

/// Tests self-messaging pattern.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Self-Messaging"
#[acton_test]
async fn test_self_messaging() -> anyhow::Result<()> {
    #[acton_actor]
    struct ProcessState {
        step: u32,
    }

    #[acton_message]
    struct StartProcess;

    #[acton_message]
    struct ProcessStep2;

    let final_step = Arc::new(AtomicU32::new(0));
    let final_step_clone = final_step.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<ProcessState>();

    actor
        .mutate_on::<StartProcess>(|actor, _ctx| {
            actor.model.step = 1;
            let self_handle = actor.handle().clone();

            Reply::pending(async move {
                // Do some work...

                // Then send a message to self for the next step
                self_handle.send(ProcessStep2).await;
            })
        })
        .mutate_on::<ProcessStep2>(|actor, _ctx| {
            actor.model.step = 2;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_step_clone.store(actor.model.step, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;
    handle.send(StartProcess).await;

    runtime.shutdown_all().await?;

    assert_eq!(final_step.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Tests request-reply pattern.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Request-Reply"
#[acton_test]
async fn test_request_reply_pattern() -> anyhow::Result<()> {
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

    // Create account (responder)
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

/// Tests command pattern with enum message.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Command Pattern"
#[acton_test]
async fn test_command_pattern() -> anyhow::Result<()> {
    #[acton_actor]
    struct Machine {
        running: bool,
        value: u32,
    }

    #[acton_message]
    enum Command {
        Start,
        Stop,
        Reset,
        SetValue(u32),
    }

    let final_state = Arc::new(AtomicU32::new(0));
    let final_state_clone = final_state.clone();
    let was_running = Arc::new(AtomicBool::new(false));
    let was_running_clone = was_running.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Machine>();

    actor
        .mutate_on::<Command>(|actor, ctx| {
            match ctx.message() {
                Command::Start => actor.model.running = true,
                Command::Stop => actor.model.running = false,
                Command::Reset => actor.model.value = 0,
                Command::SetValue(v) => actor.model.value = *v,
            }
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_state_clone.store(actor.model.value, Ordering::SeqCst);
            was_running_clone.store(actor.model.running, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(Command::Start).await;
    handle.send(Command::SetValue(42)).await;
    handle.send(Command::Stop).await;

    runtime.shutdown_all().await?;

    assert_eq!(final_state.load(Ordering::SeqCst), 42);
    assert!(!was_running.load(Ordering::SeqCst));

    Ok(())
}

/// Tests forwarding messages pattern.
///
/// From: docs/core-concepts/messages-and-handlers/page.md - "Forwarding Messages"
#[acton_test]
async fn test_forwarding_messages() -> anyhow::Result<()> {
    #[acton_actor]
    struct Router {
        worker: Option<ActorHandle>,
    }

    #[acton_actor]
    struct Worker {
        received: bool,
    }

    #[acton_message]
    struct TaskRequest {
        id: u32,
    }

    /// Asks an actor whether it has dealt with the task yet.
    #[acton_message]
    struct WasReceived;

    #[acton_message]
    struct Received(bool);

    impl Request for WasReceived {
        type Response = Received;
    }

    let mut runtime = ActonApp::launch_async().await;

    // Create worker
    let mut worker = runtime.new_actor::<Worker>();
    worker
        .mutate_on::<TaskRequest>(|actor, _ctx| {
            actor.model.received = true;
            Reply::ready()
        })
        .mutate_on::<WasReceived>(|actor, ctx| {
            let reply = ctx.reply_envelope();
            let received = actor.model.received;
            Reply::pending(async move {
                reply.send(Received(received)).await;
            })
        });
    let worker_handle = worker.start().await;

    // Create router
    let mut router = runtime.new_actor::<Router>();
    router.model.worker = Some(worker_handle.clone());

    router
        .mutate_on::<TaskRequest>(|actor, ctx| {
            // Pick a worker and forward
            let worker = actor.model.worker.clone().unwrap();
            let task = ctx.message().clone();

            Reply::pending(async move {
                worker.send(task).await;
            })
        })
        .mutate_on::<WasReceived>(|_actor, ctx| {
            let reply = ctx.reply_envelope();
            Reply::pending(async move {
                reply.send(Received(true)).await;
            })
        });

    let router_handle = router.start().await;

    router_handle.send(TaskRequest { id: 1 }).await;

    // Two hops, so two barriers. The router's answer proves it has already forwarded,
    // because a mutable handler is awaited before the actor takes its next message - so
    // the task is in the worker's inbox by the time that reply lands. The question to
    // the worker is therefore queued behind the task in its own FIFO inbox, and its
    // answer proves the task was handled.
    let _: Received = router_handle.ask(WasReceived).await?;
    let received: Received = worker_handle.ask(WasReceived).await?;
    assert!(received.0);

    runtime.shutdown_all().await?;

    Ok(())
}
