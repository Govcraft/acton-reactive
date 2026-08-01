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

#![allow(dead_code, unused_doc_comments)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tracing::*;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Use direct paths as re-exports seem problematic in test context
use crate::setup::{
    actors::{comedian::Comedian, counter::Counter},
    initialize_tracing,
    messages::{Ping, Pong},
};

mod setup;

/// Test actor model that records received broadcasts in shared counters,
/// allowing assertions in the test body (rather than in lifecycle hooks).
#[derive(Default, Debug)]
struct Recorder {
    pings: Arc<AtomicUsize>,
    pongs: Arc<AtomicUsize>,
}

/// Tests the basic publish-subscribe functionality of the `ActorBroker`.
///
/// **Scenario:**
/// 1. Launch the runtime and get the central broker handle.
/// 2. Create two actors: `Comedian` and `Counter`. The `Counter` is configured with the broker handle.
/// 3. Configure handlers:
///    - `Counter` handles `Pong` (increments count). Asserts count is 1 on stop.
///    - `Comedian` handles `Ping` and `Pong` (increments `funny` count). Asserts `funny` is 2 on stop.
/// 4. Subscribe actors to message types using their handles:
///    - `Counter` subscribes to `Pong`.
///    - `Comedian` subscribes to `Ping` and `Pong`.
/// 5. Start both actors.
/// 6. Broadcast `Ping` and `Pong` messages using the broker handle obtained from the started `Comedian` actor.
/// 7. Shut down the runtime (which stops all actors and triggers `after_stop` handlers).
///
/// **Verification:**
/// - `Comedian` receives `Ping` (funny=1) and `Pong` (funny=2). `after_stop` asserts `funny == 2`.
/// - `Counter` receives `Pong` (count=1). `after_stop` asserts `count == 1`.
#[acton_test]
async fn test_broker() -> anyhow::Result<()> {
    initialize_tracing();
    // Launch the runtime environment.
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    // Get a handle to the central message broker.
    let broker_handle = runtime.broker();

    // --- Comedian Actor ---
    // Create the actor builder. No broker needed in config as it gets it automatically.
    let mut comedian_actor_builder = runtime.new_actor::<Comedian>();

    // --- Counter Actor ---
    // Configure the counter actor, explicitly providing the broker handle.
    // Provide broker handle for potential direct use (though subscribe uses handle's broker).
    let counter_config = ActorConfig::new(
        Ern::with_root("counter").unwrap(),
        Some(broker_handle.clone()),
    );
    let mut counter_actor_builder = runtime.new_actor_with_config::<Counter>(counter_config);

    // Configure Counter actor's handlers.
    counter_actor_builder
        .mutate_on::<Pong>(|actor, _envelope| {
            info!("Also SUCCESS! PONG!");
            actor.model.count += 1;
            Reply::ready()
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.count, 1, "count should be 1");
            Reply::ready()
        });

    // Configure Comedian actor's handlers.
    comedian_actor_builder
        .mutate_on::<Ping>(|actor, _envelope| {
            info!("SUCCESS! PING!");
            actor.model.funny += 1;
            Reply::ready()
        })
        .mutate_on::<Pong>(|actor, _envelope| {
            actor.model.funny += 1;
            info!("SUCCESS! PONG!");
            Reply::ready()
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.funny, 2, "funny count should be 2");
            Reply::ready()
        });

    // Subscribe actors to messages *before* starting them.
    // The `subscribe` method uses the broker reference stored within the actor's handle.
    counter_actor_builder.handle().subscribe::<Pong>().await;
    comedian_actor_builder.handle().subscribe::<Ping>().await;
    comedian_actor_builder.handle().subscribe::<Pong>().await;

    // Start the actors.
    let comedian_handle = comedian_actor_builder.start().await;
    let _counter_handle = counter_actor_builder.start().await; // Handle not used directly after start

    // Broadcast messages using the broker reference held by the comedian's handle.
    // Any actor subscribed to these message types will receive them.
    if let Some(broker_ref) = comedian_handle.broker.as_ref() {
        broker_ref.broadcast(Ping).await; // Should be received by Comedian
        broker_ref.broadcast(Pong).await; // Should be received by Comedian and Counter
    }

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Shut down the runtime, stopping all actors and running `after_stop` handlers.
    runtime.shutdown_all().await?;

    Ok(())
}

#[acton_test]
async fn test_broker_from_handler() -> anyhow::Result<()> {
    /// Tests broadcasting a message from within an actor's message handler.
    ///
    /// **Scenario:**
    /// 1. Launch runtime, get broker handle.
    /// 2. Create `Comedian` and `Counter` actors.
    /// 3. Configure handlers:
    ///    - `Counter` handles `Pong` (increments count). Asserts count is 1 on stop.
    ///    - `Comedian` handles `Ping`:
    ///        - Gets its own broker handle using `actor.broker()`.
    ///        - Asynchronously broadcasts a `Pong` message.
    /// 4. Subscribe actors:
    ///    - `Counter` subscribes to `Pong`.
    ///    - `Comedian` subscribes to `Ping`.
    /// 5. Start both actors.
    /// 6. Broadcast an initial `Ping` message using the main broker handle.
    /// 7. Shut down the runtime.
    ///
    /// **Verification:**
    /// - Initial `Ping` broadcast is received by `Comedian`.
    /// - `Comedian`'s `Ping` handler broadcasts `Pong`.
    /// - `Pong` broadcast is received by `Counter` (count=1).
    /// - `Counter`'s `after_stop` handler asserts `count == 1`.
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let broker_handle = runtime.broker();

    // --- Comedian Actor ---
    let mut comedian_actor_builder = runtime.new_actor::<Comedian>();

    // --- Counter Actor ---
    let counter_config = ActorConfig::new(
        Ern::with_root("counter").unwrap(),
        Some(broker_handle.clone()),
    );
    let mut counter_actor_builder = runtime.new_actor_with_config::<Counter>(counter_config);

    // Configure Counter handler.
    counter_actor_builder
        .mutate_on::<Pong>(|actor, _envelope| {
            info!("Also SUCCESS! PONG!");
            actor.model.count += 1;
            Reply::ready()
        })
        .after_stop(|actor| {
            assert_eq!(actor.model.count, 1, "count should be 1");
            Reply::ready()
        });

    // Configure Comedian handler to broadcast from within.
    comedian_actor_builder.mutate_on::<Ping>(|actor, _envelope| {
        // Get the broker handle associated with this actor.
        let actor_broker_handle = actor.broker().clone();
        // Return an Reply to perform the broadcast.
        Reply::pending(async move {
            // Broadcast Pong using the handle obtained within the actor context.
            actor_broker_handle.broadcast(Pong).await;
        })
    });

    // Subscribe actors.
    counter_actor_builder.handle().subscribe::<Pong>().await; // Counter listens for Pong
    comedian_actor_builder.handle().subscribe::<Ping>().await; // Comedian listens for Ping

    // Start actors.
    let _comedian_handle = comedian_actor_builder.start().await;
    let _counter_handle = counter_actor_builder.start().await;

    // Initiate the sequence by broadcasting Ping using the main runtime broker handle.
    // This will trigger the Comedian's Ping handler, which will then broadcast Pong.
    broker_handle.broadcast(Ping).await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Shut down the runtime.
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests that `unsubscribe_async` stops broadcast delivery for the
/// unsubscribed type while leaving the actor's other subscriptions (and other
/// subscribers of the same type) intact.
///
/// **Scenario:**
/// 1. Launch the runtime and get the central broker handle.
/// 2. Create a `Recorder` actor that subscribes to `Ping` and `Pong`, then
///    unsubscribes from `Ping` again before starting.
/// 3. Create a second `Recorder` actor that stays subscribed to `Ping`.
/// 4. Broadcast one `Ping` and one `Pong`.
///
/// **Verification:**
/// - The unsubscribing actor receives no `Ping` but still receives `Pong`.
/// - The second actor still receives `Ping`, proving only the requesting
///   actor's subscription was removed.
#[acton_test]
async fn test_unsubscribe_stops_broadcast_delivery() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let broker_handle = runtime.broker();

    // --- Unsubscribing Recorder Actor ---
    let mut unsubscriber_builder = runtime.new_actor::<Recorder>();
    let unsubscriber_pings = unsubscriber_builder.model.pings.clone();
    let unsubscriber_pongs = unsubscriber_builder.model.pongs.clone();
    unsubscriber_builder
        .mutate_on::<Ping>(|actor, _envelope| {
            actor.model.pings.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        })
        .mutate_on::<Pong>(|actor, _envelope| {
            actor.model.pongs.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

    // --- Listening Recorder Actor ---
    let mut listener_builder = runtime.new_actor::<Recorder>();
    let listener_pings = listener_builder.model.pings.clone();
    listener_builder.mutate_on::<Ping>(|actor, _envelope| {
        actor.model.pings.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });

    // Subscribe to both message types, then drop the Ping subscription again.
    unsubscriber_builder.handle().subscribe::<Ping>().await;
    unsubscriber_builder.handle().subscribe::<Pong>().await;
    listener_builder.handle().subscribe::<Ping>().await;
    unsubscriber_builder
        .handle()
        .unsubscribe_async::<Ping>()
        .await;

    let _unsubscriber_handle = unsubscriber_builder.start().await;
    let _listener_handle = listener_builder.start().await;

    // Broadcast both message types.
    broker_handle.broadcast(Ping).await;
    broker_handle.broadcast(Pong).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(
        unsubscriber_pings.load(Ordering::SeqCst),
        0,
        "unsubscribed actor should no longer receive Ping broadcasts"
    );
    assert_eq!(
        unsubscriber_pongs.load(Ordering::SeqCst),
        1,
        "other subscriptions of the unsubscribing actor should still deliver"
    );
    assert_eq!(
        listener_pings.load(Ordering::SeqCst),
        1,
        "other subscribers of the unsubscribed type should be unaffected"
    );

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests that the fire-and-forget `unsubscribe` also removes the subscription.
///
/// **Scenario:**
/// 1. Create a `Recorder` actor subscribed to `Ping` and call the
///    unit-returning `unsubscribe::<Ping>()` (no `.await`).
/// 2. Give the spawned send task time to reach the broker, then broadcast a
///    `Ping`.
///
/// **Verification:**
/// - The actor receives no `Ping`, proving the fire-and-forget path sends a
///   fully populated removal request rather than being a no-op.
#[acton_test]
async fn test_fire_and_forget_unsubscribe_removes_subscription() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let broker_handle = runtime.broker();

    let mut recorder_builder = runtime.new_actor::<Recorder>();
    let recorder_pings = recorder_builder.model.pings.clone();
    recorder_builder.mutate_on::<Ping>(|actor, _envelope| {
        actor.model.pings.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });

    recorder_builder.handle().subscribe::<Ping>().await;
    recorder_builder.handle().unsubscribe::<Ping>();
    let _recorder_handle = recorder_builder.start().await;

    // The fire-and-forget call queues the removal on a background task; give
    // it time to reach the broker before broadcasting.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    broker_handle.broadcast(Ping).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(
        recorder_pings.load(Ordering::SeqCst),
        0,
        "fire-and-forget unsubscribe should stop Ping delivery"
    );

    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests that a stopped actor is automatically removed from the broker's
/// subscriber map.
///
/// **Scenario:**
/// 1. Start a `Recorder` actor with an explicit `Ern`, subscribed to `Pong`,
///    then stop it.
/// 2. Start a second `Recorder` actor reusing the *same* `Ern` and subscribe
///    it to `Pong` as well.
/// 3. Broadcast a `Pong`.
///
/// **Verification:**
/// - The broker's subscriber set is keyed by `(Ern, ActorHandle)` where handle
///   equality is id-based. If the stopped actor's entry leaked, the second
///   subscription (an equal key) would be a no-op and the broadcast would go
///   to the dead handle. Receiving the broadcast in the second incarnation
///   proves the stopped actor's entry was removed on termination.
#[acton_test]
async fn test_stopped_actor_is_unsubscribed_from_broker() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let broker_handle = runtime.broker();

    let recorder_id = Ern::with_root("recorder").unwrap();

    // --- First incarnation: subscribe, start, then stop ---
    let first_config = ActorConfig::new(recorder_id.clone(), Some(broker_handle.clone()));
    let mut first_builder = runtime.new_actor_with_config::<Recorder>(first_config);
    let first_pongs = first_builder.model.pongs.clone();
    first_builder.mutate_on::<Pong>(|actor, _envelope| {
        actor.model.pongs.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });
    first_builder.handle().subscribe::<Pong>().await;
    let first_handle = first_builder.start().await;
    first_handle.stop().await?;

    // --- Second incarnation: same Ern, fresh actor and subscription ---
    let second_config = ActorConfig::new(recorder_id, Some(broker_handle.clone()));
    let mut second_builder = runtime.new_actor_with_config::<Recorder>(second_config);
    let second_pongs = second_builder.model.pongs.clone();
    second_builder.mutate_on::<Pong>(|actor, _envelope| {
        actor.model.pongs.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });
    second_builder.handle().subscribe::<Pong>().await;
    let _second_handle = second_builder.start().await;

    // Broadcast after the first incarnation stopped.
    broker_handle.broadcast(Pong).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    assert_eq!(
        second_pongs.load(Ordering::SeqCst),
        1,
        "the replacement actor should receive broadcasts, proving the stopped \
         actor's subscription entry was removed"
    );
    assert_eq!(
        first_pongs.load(Ordering::SeqCst),
        0,
        "the stopped actor should not have received any broadcasts"
    );

    runtime.shutdown_all().await?;

    Ok(())
}
