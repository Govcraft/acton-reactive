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

//! Tests for examples from docs/handler-types/page.md
//!
//! This module verifies that the code examples from the "Handler Types"
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Instant;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests `mutate_on` handler - exclusive mutable access.
///
/// From: docs/handler-types/page.md - "`mutate_on`"
#[acton_test]
async fn test_mutate_on_handler() -> anyhow::Result<()> {
    #[acton_actor]
    struct Counter {
        counter: u32,
        last_updated: Option<Instant>,
    }

    #[acton_message]
    struct UpdateCounter {
        increment: u32,
    }

    let final_count = Arc::new(AtomicU32::new(0));
    let final_count_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Counter>();

    actor
        .mutate_on::<UpdateCounter>(|actor, ctx| {
            // Mutable access to state
            actor.model.counter += ctx.message().increment;
            actor.model.last_updated = Some(Instant::now());

            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.counter, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    handle.send(UpdateCounter { increment: 5 }).await;
    handle.send(UpdateCounter { increment: 10 }).await;
    handle.send(UpdateCounter { increment: 3 }).await;

    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 18);

    Ok(())
}

/// Tests `act_on` handler - shared read-only access.
///
/// From: docs/handler-types/page.md - "`act_on`"
#[acton_test]
async fn test_act_on_handler() -> anyhow::Result<()> {
    #[acton_actor]
    struct StatusHolder {
        status: String,
    }

    #[acton_message]
    struct GetStatus;

    #[acton_message]
    struct StatusResponse(String);

    // Naming the reply type is what makes `GetStatus` usable with `ask`.
    impl Request for GetStatus {
        type Response = StatusResponse;
    }

    let mut runtime = ActonApp::launch_async().await;

    let mut holder = runtime.new_actor::<StatusHolder>();
    holder.model.status = "healthy".to_string();

    holder.act_on::<GetStatus>(|actor, ctx| {
        // Read-only access to state
        let status = actor.model.status.clone();
        let reply = ctx.reply_envelope();

        Reply::pending(async move {
            reply.send(StatusResponse(status)).await;
        })
    });

    let holder_handle = holder.start().await;

    // `ask` resolves once the handler has answered, so the reply is in hand before the
    // next line runs. Nothing here depends on how long the actor took.
    let status: StatusResponse = holder_handle.ask(GetStatus).await?;
    assert_eq!(status.0, "healthy");

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests `try_mutate_on` handler - mutable access with error handling.
///
/// From: docs/handler-types/page.md - "`try_mutate_on`"
#[acton_test]
async fn test_try_mutate_on_handler() -> anyhow::Result<()> {
    #[acton_actor]
    struct Account {
        balance: u64,
    }

    #[acton_message]
    struct ProcessPayment {
        amount: u64,
    }

    #[derive(Debug, Clone)]
    struct InsufficientFunds {
        balance: u64,
        required: u64,
    }

    impl std::fmt::Display for InsufficientFunds {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "Insufficient funds: balance {}, required {}",
                self.balance, self.required
            )
        }
    }

    impl std::error::Error for InsufficientFunds {}

    #[derive(Debug, Clone)]
    struct PaymentSuccess {
        remaining: u64,
    }

    let final_balance = Arc::new(AtomicU32::new(0));
    let final_balance_clone = final_balance.clone();
    let error_occurred = Arc::new(AtomicBool::new(false));
    let error_clone = error_occurred.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Account>();
    actor.model.balance = 100;

    actor
        .try_mutate_on::<ProcessPayment, PaymentSuccess, InsufficientFunds>(|actor, ctx| {
            let amount = ctx.message().amount;
            let balance = actor.model.balance;

            if balance < amount {
                // Immediate error
                Reply::try_err(InsufficientFunds {
                    balance,
                    required: amount,
                })
            } else {
                actor.model.balance -= amount;
                // Immediate success
                Reply::try_ok(PaymentSuccess {
                    remaining: actor.model.balance,
                })
            }
        })
        .on_error::<ProcessPayment, InsufficientFunds>(move |_actor, _ctx, _error| {
            error_clone.store(true, Ordering::SeqCst);
            Box::pin(async {})
        })
        .after_stop(move |actor| {
            final_balance_clone.store(
                u32::try_from(actor.model.balance).unwrap_or(u32::MAX),
                Ordering::SeqCst,
            );
            Reply::ready()
        });

    let handle = actor.start().await;

    // Successful payment
    handle.send(ProcessPayment { amount: 30 }).await;
    // This should fail - not enough balance
    handle.send(ProcessPayment { amount: 100 }).await;
    // Another successful payment
    handle.send(ProcessPayment { amount: 20 }).await;

    runtime.shutdown_all().await?;

    // 100 - 30 - 20 = 50 (the 100 payment failed)
    assert_eq!(final_balance.load(Ordering::SeqCst), 50);
    assert!(error_occurred.load(Ordering::SeqCst));

    Ok(())
}

/// Tests `try_act_on` handler - read-only access with error handling.
///
/// From: docs/handler-types/page.md - "`try_act_on`"
#[acton_test]
async fn test_try_act_on_handler() -> anyhow::Result<()> {
    use std::collections::HashMap;

    #[acton_actor]
    struct Cache {
        data: HashMap<String, String>,
        lookups: u32,
    }

    #[acton_message]
    struct CheckCache {
        key: String,
    }

    #[acton_message]
    struct LookupDone;

    #[derive(Debug, Clone)]
    struct CacheHit {
        value: String,
    }

    #[derive(Debug, Clone)]
    struct CacheMiss {
        key: String,
    }

    impl std::fmt::Display for CacheMiss {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Cache miss for key: {}", self.key)
        }
    }

    impl std::error::Error for CacheMiss {}

    let lookups_done = Arc::new(AtomicU32::new(0));
    let lookups_clone = lookups_done.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Cache>();
    actor
        .model
        .data
        .insert("exists".to_string(), "value".to_string());

    actor
        .try_act_on::<CheckCache, CacheHit, CacheMiss>(|actor, ctx| {
            let key = ctx.message().key.clone();
            let self_handle = actor.handle().clone();

            match actor.model.data.get(&key) {
                Some(value) => {
                    let value = value.clone();
                    Reply::try_pending(async move {
                        self_handle.send(LookupDone).await;
                        Ok(CacheHit { value })
                    })
                }
                None => Reply::try_pending(async move {
                    self_handle.send(LookupDone).await;
                    Err(CacheMiss { key })
                }),
            }
        })
        .mutate_on::<LookupDone>(|actor, _ctx| {
            actor.model.lookups += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            lookups_clone.store(actor.model.lookups, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    // This should hit
    handle
        .send(CheckCache {
            key: "exists".to_string(),
        })
        .await;
    // This should miss
    handle
        .send(CheckCache {
            key: "missing".to_string(),
        })
        .await;

    runtime.shutdown_all().await?;

    // Both lookups should have been counted
    assert_eq!(lookups_done.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Tests batching messages for performance.
///
/// From: docs/handler-types/page.md - "Batch Related Operations"
#[acton_test]
async fn test_batch_operations() -> anyhow::Result<()> {
    #[acton_actor]
    struct Processor {
        processed_count: u32,
    }

    // Use a batch message instead of many individual messages
    #[acton_message]
    struct ProcessBatch {
        items: Vec<String>,
    }

    let final_count = Arc::new(AtomicU32::new(0));
    let final_count_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Processor>();

    actor
        .mutate_on::<ProcessBatch>(|actor, ctx| {
            for _item in &ctx.message().items {
                // Process each item
                actor.model.processed_count += 1;
            }
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_count_clone.store(actor.model.processed_count, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    // Send batch of items
    handle
        .send(ProcessBatch {
            items: vec![
                "item1".to_string(),
                "item2".to_string(),
                "item3".to_string(),
                "item4".to_string(),
                "item5".to_string(),
            ],
        })
        .await;

    runtime.shutdown_all().await?;

    assert_eq!(final_count.load(Ordering::SeqCst), 5);

    Ok(())
}

/// Tests keeping handlers lightweight by offloading heavy work.
///
/// From: docs/handler-types/page.md - "Keep Handlers Lightweight"
#[acton_test]
async fn test_lightweight_handlers() -> anyhow::Result<()> {
    #[acton_actor]
    struct Processor {
        completed: bool,
    }

    #[acton_message]
    struct Process {
        data: String,
    }

    #[acton_message]
    struct ProcessComplete;

    /// Sent back once the offloaded work has finished *and* `ProcessComplete` has been
    /// queued. See the two-step wait below for why that ordering is the whole point.
    #[acton_message]
    struct Offloaded;

    impl Request for Process {
        type Response = Offloaded;
    }

    #[acton_message]
    struct GetStatus;

    #[acton_message]
    struct Status {
        completed: bool,
    }

    impl Request for GetStatus {
        type Response = Status;
    }

    let completed = Arc::new(AtomicBool::new(false));
    let completed_clone = completed.clone();

    let mut runtime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<Processor>();

    actor
        .mutate_on::<Process>(|actor, ctx| {
            let data = ctx.message().data.clone();
            let self_handle = actor.handle().clone();
            let reply = ctx.reply_envelope();

            // Good: offload heavy work to async
            Reply::pending(async move {
                // Simulate heavy computation in a blocking task
                let _result = tokio::task::spawn_blocking(move || {
                    // Expensive computation would happen here
                    data.len()
                })
                .await
                .unwrap();

                // Notify self when done
                self_handle.send(ProcessComplete).await;

                // Reply only *after* `ProcessComplete` is queued, so a caller that has
                // received this reply knows the follow-up message is already in the
                // inbox ahead of anything it sends next.
                reply.send(Offloaded).await;
            })
        })
        .mutate_on::<ProcessComplete>(|actor, _ctx| {
            actor.model.completed = true;
            Reply::ready()
        })
        .mutate_on::<GetStatus>(|actor, ctx| {
            let reply = ctx.reply_envelope();
            let completed = actor.model.completed;
            Reply::pending(async move {
                reply.send(Status { completed }).await;
            })
        })
        .after_stop(move |actor| {
            completed_clone.store(actor.model.completed, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    // Two asks, not one. `Process` hands its heavy work to a pending future and only
    // then self-sends `ProcessComplete`, so `ProcessComplete` is queued *after* the
    // handler was entered. A single `ask(GetStatus)` sent straight after `send(Process)`
    // would sit ahead of `ProcessComplete` in the inbox and observe `completed == false`
    // — the original race, merely wearing a nicer API.
    //
    // Asking `Process` itself fixes the ordering: its reply is sent from inside the same
    // pending future, after the self-send. Once it returns, `ProcessComplete` is already
    // queued, so the FIFO inbox puts the second ask strictly behind it.
    let _: Offloaded = handle
        .ask(Process {
            data: "test data".to_string(),
        })
        .await?;

    let status: Status = handle.ask(GetStatus).await?;
    assert!(
        status.completed,
        "the self-sent ProcessComplete must have been handled before this ask"
    );

    runtime.shutdown_all().await?;

    assert!(completed.load(Ordering::SeqCst));

    Ok(())
}
