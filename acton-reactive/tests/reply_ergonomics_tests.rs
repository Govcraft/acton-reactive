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

//! Tests demonstrating ergonomic `Reply::pending` usage without explicit `async move` blocks.
//!
//! This test file proves that when you have a function returning a Future,
//! you can pass it directly to `Reply::pending()` without wrapping in `async move { ... }`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

use crate::setup::{actors::pool_item::PoolItem, initialize_tracing, messages::Ping};

mod setup;

/// A helper async function that simulates some async work.
/// This returns a `Future<Output = ()>` which can be passed directly to `Reply::pending()`.
async fn do_async_work(counter: Arc<AtomicUsize>) {
    // Simulate some async operation
    tokio::task::yield_now().await;
    counter.fetch_add(1, Ordering::SeqCst);
}

/// Another helper that returns a future - this one takes no arguments
async fn simple_async_task() {
    tokio::task::yield_now().await;
}

/// Tests that `Reply::pending()` can accept a future directly without async move wrapper.
///
/// **Key insight**: When you have `async fn foo() { ... }`, calling `foo()` returns
/// a `Future<Output = ()>`. You can pass this directly to `Reply::pending(foo())`
/// instead of writing `Reply::pending(async move { foo().await })`.
///
/// **Before (verbose)**:
/// ```ignore
/// Reply::pending(async move {
///     do_async_work(counter).await;
/// })
/// ```
///
/// **After (ergonomic)**:
/// ```ignore
/// Reply::pending(do_async_work(counter))
/// ```
#[acton_test]
async fn test_reply_pending_accepts_future_directly() -> anyhow::Result<()> {
    initialize_tracing();

    let async_work_counter = Arc::new(AtomicUsize::new(0));
    let counter_for_handler = async_work_counter.clone();

    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<PoolItem>();

    actor
        .mutate_on::<Ping>(move |actor, _ctx| {
            actor.model.receive_count += 1;
            let counter = counter_for_handler.clone();

            // THIS IS THE KEY: passing the future directly without `async move { ... }`
            Reply::pending(do_async_work(counter))
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.receive_count, 3, "expected three Pings");
            async { }
        });

    let handle = actor.start().await;

    // Send 3 messages
    handle.send(Ping).await;
    handle.send(Ping).await;
    handle.send(Ping).await;

    handle.stop().await?;

    // Verify the async work was actually executed
    assert_eq!(
        async_work_counter.load(Ordering::SeqCst),
        3,
        "async work should have been executed 3 times"
    );

    Ok(())
}

/// Tests that `Reply::pending()` works with simple async functions that take no arguments.
#[acton_test]
async fn test_reply_pending_with_simple_async_fn() -> anyhow::Result<()> {
    initialize_tracing();

    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let mut actor = runtime.new_actor::<PoolItem>();

    actor
        .mutate_on::<Ping>(|actor, _ctx| {
            actor.model.receive_count += 1;

            // Passing a simple async function directly
            Reply::pending(simple_async_task())
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.receive_count, 1, "expected one Ping");
            async { }
        });

    let handle = actor.start().await;
    handle.send(Ping).await;
    handle.stop().await?;

    Ok(())
}

/// Tests comparing the old verbose style vs the new ergonomic style.
/// Both should work identically.
#[acton_test]
async fn test_reply_pending_old_vs_new_style() -> anyhow::Result<()> {
    initialize_tracing();

    let counter_old = Arc::new(AtomicUsize::new(0));
    let counter_new = Arc::new(AtomicUsize::new(0));

    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    // Actor using OLD verbose style
    let counter_for_old = counter_old.clone();
    let mut old_style_actor = runtime.new_actor::<PoolItem>();
    old_style_actor.mutate_on::<Ping>(move |_actor, _ctx| {
        let counter = counter_for_old.clone();
        // OLD STYLE: explicit async move wrapper
        Reply::pending(async move {
            do_async_work(counter).await;
        })
    });
    let old_handle = old_style_actor.start().await;

    // Actor using NEW ergonomic style
    let counter_for_new = counter_new.clone();
    let mut new_style_actor = runtime.new_actor::<PoolItem>();
    new_style_actor.mutate_on::<Ping>(move |_actor, _ctx| {
        let counter = counter_for_new.clone();
        // NEW STYLE: pass the future directly
        Reply::pending(do_async_work(counter))
    });
    let new_handle = new_style_actor.start().await;

    // Send messages to both
    old_handle.send(Ping).await;
    new_handle.send(Ping).await;

    old_handle.stop().await?;
    new_handle.stop().await?;

    // Both should have executed their async work exactly once
    assert_eq!(counter_old.load(Ordering::SeqCst), 1, "old style should work");
    assert_eq!(counter_new.load(Ordering::SeqCst), 1, "new style should work");

    Ok(())
}
