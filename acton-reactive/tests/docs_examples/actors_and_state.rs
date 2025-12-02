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

//! Tests for examples from docs/actors-and-state/page.md
//!
//! This module verifies that the code examples from the "Actors & State"
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests defining actor state with complex fields.
///
/// From: docs/actors-and-state/page.md - "Defining Actor State"
#[acton_test]
async fn test_complex_actor_state() -> anyhow::Result<()> {
    #[acton_actor]
    struct ShoppingCart {
        items: Vec<String>,
        total: f64,
        customer_id: Option<String>,
    }

    #[acton_message]
    struct AddItem {
        name: String,
        price: f64,
    }

    let final_total = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let final_total_clone = final_total.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut cart = runtime.new_actor::<ShoppingCart>();

    cart.mutate_on::<AddItem>(|actor, ctx| {
        actor.model.items.push(ctx.message().name.clone());
        actor.model.total += ctx.message().price;
        Reply::ready()
    })
    .after_stop(move |actor| {
        // Store total as bits for comparison
        final_total_clone.store(actor.model.total.to_bits(), Ordering::SeqCst);
        Reply::ready()
    });

    let handle = cart.start().await;

    handle
        .send(AddItem {
            name: "Apple".to_string(),
            price: 1.50,
        })
        .await;
    handle
        .send(AddItem {
            name: "Banana".to_string(),
            price: 0.75,
        })
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    let total = f64::from_bits(final_total.load(Ordering::SeqCst));
    assert!((total - 2.25).abs() < f64::EPSILON);

    Ok(())
}

/// Tests using no_default for custom initialization.
///
/// From: docs/actors-and-state/page.md - "Custom Default for Complex State"
#[acton_test]
async fn test_custom_default_actor() -> anyhow::Result<()> {
    // Note: We can't test Stdout directly, so we use a simpler example
    // that demonstrates the no_default pattern

    #[acton_actor(no_default)]
    struct ConfiguredActor {
        config_value: String,
        start_time: Instant,
    }

    impl Default for ConfiguredActor {
        fn default() -> Self {
            Self {
                config_value: "custom_default".to_string(),
                start_time: Instant::now(),
            }
        }
    }

    #[acton_message]
    struct GetConfig;

    let config_verified = Arc::new(AtomicBool::new(false));
    let config_verified_clone = config_verified.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<ConfiguredActor>();

    actor
        .act_on::<GetConfig>(move |actor, _ctx| {
            // Verify the custom default was used
            assert_eq!(actor.model.config_value, "custom_default");
            Reply::ready()
        })
        .after_stop(move |actor| {
            config_verified_clone.store(
                actor.model.config_value == "custom_default",
                Ordering::SeqCst,
            );
            Reply::ready()
        });

    let handle = actor.start().await;
    handle.send(GetConfig).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert!(config_verified.load(Ordering::SeqCst));

    Ok(())
}

/// Tests accessing state in handlers.
///
/// From: docs/actors-and-state/page.md - "Accessing State in Handlers"
#[acton_test]
async fn test_accessing_state_in_handlers() -> anyhow::Result<()> {
    #[acton_actor]
    struct Account {
        balance: f64,
        last_updated: Option<Instant>,
    }

    #[acton_message]
    struct UpdateBalance {
        amount: f64,
    }

    let final_balance = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let final_balance_clone = final_balance.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Account>();

    actor
        .mutate_on::<UpdateBalance>(|actor, ctx| {
            // Read state
            let _current = actor.model.balance;

            // Modify state
            actor.model.balance += ctx.message().amount;
            actor.model.last_updated = Some(Instant::now());

            Reply::ready()
        })
        .after_stop(move |actor| {
            final_balance_clone.store(actor.model.balance.to_bits(), Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(UpdateBalance { amount: 100.0 }).await;
    handle.send(UpdateBalance { amount: 50.0 }).await;
    handle.send(UpdateBalance { amount: -25.0 }).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    let balance = f64::from_bits(final_balance.load(Ordering::SeqCst));
    assert!((balance - 125.0).abs() < f64::EPSILON);

    Ok(())
}

/// Tests mutable vs read-only access.
///
/// From: docs/actors-and-state/page.md - "Mutable vs Read-Only Access"
///
/// Note: Request-reply requires using ctx.new_envelope() with a trigger pattern.
#[acton_test]
async fn test_mutable_vs_readonly_access() -> anyhow::Result<()> {
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
    struct Deposit {
        amount: f64,
    }

    #[acton_message]
    struct QueryBalance;

    #[acton_message]
    struct GetBalance;

    #[acton_message]
    struct BalanceResponse(f64);

    let received_balance = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let received_clone = received_balance.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Create account actor
    let mut account = runtime.new_actor::<Account>();

    // Mutable access - one at a time
    account.mutate_on::<Deposit>(|actor, ctx| {
        actor.model.balance += ctx.message().amount; // Can modify
        Reply::ready()
    });

    // Read-only access - can run concurrently
    account.act_on::<GetBalance>(|actor, ctx| {
        let balance = actor.model.balance; // Can only read
        let reply = ctx.reply_envelope();
        Reply::pending(async move {
            reply.send(BalanceResponse(balance)).await;
        })
    });

    let account_handle = account.start().await;

    // Create client to request balance using proper reply chain
    let mut client = runtime.new_actor::<Client>();
    client.model.account_handle = Some(account_handle.clone());

    client
        .mutate_on::<QueryBalance>(|actor, ctx| {
            let target = actor.model.account_handle.clone().unwrap();
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

    // Make deposits
    account_handle.send(Deposit { amount: 100.0 }).await;
    account_handle.send(Deposit { amount: 50.0 }).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Query balance via client trigger
    client_handle.send(QueryBalance).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    let balance = f64::from_bits(received_balance.load(Ordering::SeqCst));
    assert!((balance - 150.0).abs() < f64::EPSILON);

    Ok(())
}

/// Tests creating multiple actors of the same type.
///
/// From: docs/actors-and-state/page.md - "Multiple Actors"
#[acton_test]
async fn test_multiple_actors() -> anyhow::Result<()> {
    #[acton_actor]
    struct Worker {
        task_count: u32,
    }

    #[acton_message]
    struct Task {
        id: u32,
    }

    let total_tasks = Arc::new(AtomicU32::new(0));

    let mut runtime = ActonApp::launch_async().await;

    // Create multiple actors of the same type
    let mut handles = Vec::new();

    for i in 0..3 {
        let mut worker = runtime.new_actor::<Worker>();
        let counter = total_tasks.clone();

        worker
            .mutate_on::<Task>(|actor, _ctx| {
                actor.model.task_count += 1;
                Reply::ready()
            })
            .after_stop(move |actor| {
                counter.fetch_add(actor.model.task_count, Ordering::SeqCst);
                Reply::ready()
            });

        let handle = worker.start().await;
        handles.push((i, handle));
    }

    // Send to specific workers
    for (i, handle) in &handles {
        handle.send(Task { id: *i as u32 }).await;
        handle.send(Task { id: *i as u32 + 10 }).await;
    }

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    // Each worker processed 2 tasks, 3 workers total = 6
    assert_eq!(total_tasks.load(Ordering::SeqCst), 6);

    Ok(())
}

/// Tests named actors for easier debugging.
///
/// From: docs/actors-and-state/page.md - "Use Meaningful Names"
#[acton_test]
async fn test_named_actors() -> anyhow::Result<()> {
    #[acton_actor]
    struct OrderState {
        order_count: u32,
    }

    #[acton_message]
    struct ProcessOrder;

    let mut runtime = ActonApp::launch_async().await;

    // Good: clear purpose with meaningful name
    let mut order_processor =
        runtime.new_actor_with_name::<OrderState>("order-processor".to_string());

    order_processor.mutate_on::<ProcessOrder>(|actor, _ctx| {
        actor.model.order_count += 1;
        Reply::ready()
    });

    let handle = order_processor.start().await;

    // Verify the actor has a meaningful name (Ern removes hyphens and adds a suffix)
    assert!(handle.name().starts_with("orderprocessor"));

    handle.send(ProcessOrder).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests focused actor responsibility.
///
/// From: docs/actors-and-state/page.md - "Keep State Focused"
#[acton_test]
async fn test_focused_actor_state() -> anyhow::Result<()> {
    // Good: focused responsibility
    #[acton_actor]
    struct OrderProcessor {
        pending_orders: Vec<String>,
        processing_order: Option<String>,
    }

    #[acton_message]
    struct AddOrder {
        order_id: String,
    }

    #[acton_message]
    struct StartProcessing;

    let processed = Arc::new(AtomicBool::new(false));
    let processed_clone = processed.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut processor = runtime.new_actor::<OrderProcessor>();

    processor
        .mutate_on::<AddOrder>(|actor, ctx| {
            actor.model.pending_orders.push(ctx.message().order_id.clone());
            Reply::ready()
        })
        .mutate_on::<StartProcessing>(|actor, _ctx| {
            if let Some(order) = actor.model.pending_orders.pop() {
                actor.model.processing_order = Some(order);
            }
            Reply::ready()
        })
        .after_stop(move |actor| {
            processed_clone.store(actor.model.processing_order.is_some(), Ordering::SeqCst);
            Reply::ready()
        });

    let handle = processor.start().await;

    handle
        .send(AddOrder {
            order_id: "ORD-001".to_string(),
        })
        .await;
    handle.send(StartProcessing).await;

    tokio::time::sleep(Duration::from_millis(50)).await;
    runtime.shutdown_all().await?;

    assert!(processed.load(Ordering::SeqCst));

    Ok(())
}
