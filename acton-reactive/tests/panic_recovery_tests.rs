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

//! Tests for panic recovery in message handlers.
//!
//! These tests verify that panics in message handlers are caught and the actor
//! continues processing subsequent messages without crashing.
//!
//! Note: These tests use `#[tokio::test]` instead of `#[acton_test]` because
//! the `acton_test` macro's panic detection would fail the test when we
//! intentionally trigger panics to verify they are caught by the framework.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;

/// Test actor state for panic recovery tests
#[derive(Debug, Default, Clone)]
struct PanicTestActor;

/// Message that triggers a panic in the handler
#[acton_message]
struct PanicMessage;

/// Message that triggers a panic with a custom string payload
#[acton_message]
struct PanicWithMessage {
    message: String,
}

/// Normal message that increments a counter
#[acton_message]
struct IncrementCounter;

/// Message for read-only handler that panics
#[acton_message]
struct ReadOnlyPanicMessage;

/// Message for read-only handler that succeeds
#[acton_message]
struct ReadOnlySuccessMessage;

/// Test that an actor continues processing messages after a mutable handler panics.
#[tokio::test]
async fn test_actor_continues_after_mutable_handler_panic() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let mut actor = runtime.new_actor::<PanicTestActor>();
    actor
        .mutate_on::<PanicMessage>(|_actor, _ctx| {
            panic!("Intentional test panic in mutable handler");
        })
        .mutate_on::<IncrementCounter>(move |_actor, _ctx| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    // Send panic-triggering message
    handle.send(PanicMessage).await;

    // Give time for the panic to be processed
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Actor should still be alive and process subsequent messages
    handle.send(IncrementCounter).await;
    handle.send(IncrementCounter).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify actor continued processing after panic
    assert_eq!(counter.load(Ordering::SeqCst), 2, "Actor should have processed 2 IncrementCounter messages after panic");

    handle.stop().await?;
    Ok(())
}

/// Test that read-only handlers continue after one panics.
#[tokio::test]
async fn test_readonly_handlers_continue_after_panic() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let success_counter = Arc::new(AtomicU32::new(0));
    let success_counter_clone = success_counter.clone();

    let mut actor = runtime.new_actor::<PanicTestActor>();
    actor
        .act_on::<ReadOnlyPanicMessage>(|_actor, _ctx| {
            panic!("Intentional test panic in read-only handler");
        })
        .act_on::<ReadOnlySuccessMessage>(move |_actor, _ctx| {
            success_counter_clone.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    // Send panic-triggering message
    handle.send(ReadOnlyPanicMessage).await;

    // Give time for the panic to be processed
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Actor should still process subsequent read-only messages
    handle.send(ReadOnlySuccessMessage).await;
    handle.send(ReadOnlySuccessMessage).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify actor continued processing after panic
    assert_eq!(success_counter.load(Ordering::SeqCst), 2, "Actor should have processed 2 ReadOnlySuccessMessage messages after panic");

    handle.stop().await?;
    Ok(())
}

/// Test that panic message content is preserved (verifiable via logs).
#[tokio::test]
async fn test_panic_with_custom_message() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let mut actor = runtime.new_actor::<PanicTestActor>();
    actor
        .mutate_on::<PanicWithMessage>(|_actor, ctx| {
            let msg = ctx.message().message.clone();
            panic!("Custom panic: {msg}");
        })
        .mutate_on::<IncrementCounter>(move |_actor, _ctx| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    // Send panic-triggering message with custom content
    handle.send(PanicWithMessage { message: "test panic payload".to_string() }).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Actor should still be alive
    handle.send(IncrementCounter).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(counter.load(Ordering::SeqCst), 1, "Actor should continue after custom panic message");

    handle.stop().await?;
    Ok(())
}

/// Test multiple panics don't crash the actor.
#[tokio::test]
async fn test_multiple_panics_handled() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let counter = Arc::new(AtomicU32::new(0));
    let counter_clone = counter.clone();

    let mut actor = runtime.new_actor::<PanicTestActor>();
    actor
        .mutate_on::<PanicMessage>(|_actor, _ctx| {
            panic!("Repeated panic");
        })
        .mutate_on::<IncrementCounter>(move |_actor, _ctx| {
            counter_clone.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

    let handle = actor.start().await;

    // Send multiple panic messages
    handle.send(PanicMessage).await;
    handle.send(IncrementCounter).await;
    handle.send(PanicMessage).await;
    handle.send(IncrementCounter).await;
    handle.send(PanicMessage).await;
    handle.send(IncrementCounter).await;

    tokio::time::sleep(Duration::from_millis(100)).await;

    // All IncrementCounter messages should be processed despite panics
    assert_eq!(counter.load(Ordering::SeqCst), 3, "Actor should process all increment messages despite panics");

    handle.stop().await?;
    Ok(())
}
