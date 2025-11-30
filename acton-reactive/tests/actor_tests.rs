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

use std::time::Duration;

use tracing::{info, trace};

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Explicitly import necessary types from the setup module
// Use direct paths as re-exports seem problematic in test context
use crate::setup::{
    actors::{comedian::Comedian, counter::Counter, messenger::Messenger, pool_item::PoolItem},
    initialize_tracing,
    messages::{AudienceReactionMsg, FunnyJoke, FunnyJokeFor, Ping, Tally},
};
mod setup;

/// Tests the ability of an actor to handle asynchronous operations within its message handlers (`act_on`).
///
/// **Scenario:**
/// 1. A `Comedian` actor is created.
/// 2. It's configured to handle `FunnyJoke` messages:
///    - Increment its `jokes_told` counter.
///    - Asynchronously send a `Ping` message back to itself.
/// 3. It's configured to handle `AudienceReactionMsg` messages:
///    - Increment `funny` or `bombers` counters based on the reaction.
/// 4. It's configured to handle `Ping` messages (just logs).
/// 5. An `after_stop` handler is added to assert the final state (`jokes_told` count).
/// 6. The actor is started.
/// 7. Two `FunnyJoke` messages are sent to the actor.
/// 8. The actor is stopped.
///
/// **Verification:**
/// - The `after_stop` handler asserts that `jokes_told` is 2.
/// - The internal `Ping` messages are sent and handled (verified by tracing logs).
#[acton_test]
async fn test_async_reactor() -> anyhow::Result<()> {
    initialize_tracing();

    // Launch the Acton runtime environment. This provides the necessary infrastructure
    // for creating and managing actors.
    let mut runtime: ActorRuntime = ActonApp::launch();

    // Configure the actor's identity (ERN) and relationship (no parent, no broker needed here).
    let actor_config = ActorConfig::new(Ern::with_root("improve_show").unwrap(), None, None)?;
    // Create an actor builder for the `Comedian` state (`model`).
    // The builder is in an `Idle` state, ready for configuration.
    let mut comedian_actor_builder = runtime.new_actor_with_config::<Comedian>(actor_config);

    // Configure the actor's behavior by defining message handlers using `mutate_on`.
    comedian_actor_builder
        // Define a handler for messages of type `FunnyJoke`.
        // The closure receives the `ManagedActor` (giving access to `model` and `handle`)
        // and the incoming message `envelope`.
        .mutate_on::<FunnyJoke>(|actor, _envelope| {
            // Mutate the actor's internal state (`model`).
            actor.model.jokes_told += 1;
            // Clone the actor's handle. Handles are cheap to clone and allow interaction
            // with the actor (like sending messages) from outside the actor's task.
            let actor_handle = actor.handle().clone();
            // Return an Reply. `from_async` allows the handler
            // to perform asynchronous operations after the initial synchronous part.
            Reply::pending(async move {
                trace!("emitting async");
                // Send a `Ping` message back to the actor itself asynchronously.
                actor_handle.send(Ping).await;
            })
        })
        // Define a handler for `AudienceReactionMsg` messages.
        .mutate_on::<AudienceReactionMsg>(|actor, envelope| {
            trace!("Received Audience Reaction");
            // Access the message content from the envelope.
            match envelope.message() {
                AudienceReactionMsg::Chuckle => actor.model.funny += 1,
                AudienceReactionMsg::Groan => actor.model.bombers += 1,
            }
            // This handler completes synchronously, so we return `Reply::ready()`.
            Reply::ready()
        })
        // Define a handler for `Ping` messages (sent from the FunnyJoke handler).
        .mutate_on::<Ping>(|_actor, _envelope| {
            trace!("PING");
            Reply::ready()
        })
        // Define a callback function to execute *after* the actor has stopped processing messages
        // and its internal task has finished.
        .after_stop(|actor| {
            info!(
                "Jokes told at {}: {}\tFunny: {}\tBombers: {}",
                actor.id(),             // Access actor's unique ID.
                actor.model.jokes_told, // Access final state.
                actor.model.funny,
                actor.model.bombers
            );
            // Assert the final state after the actor has run.
            assert_eq!(actor.model.jokes_told, 2);
            Reply::ready()
        });

    // Transition the actor from `Idle` to `Started`. This spawns the actor's main task loop
    // and returns an `ActorHandle` for interaction.
    let comedian_handle = comedian_actor_builder.start().await;

    // Send messages to the now running actor using its handle.
    comedian_handle.send(FunnyJoke::ChickenCrossesRoad).await;
    comedian_handle.send(FunnyJoke::Pun).await;

    // Initiate the actor's shutdown sequence. This sends a `Terminate` signal.
    // The actor will finish processing any messages currently in its inbox,
    // then execute its `after_stop` handler before fully terminating.
    // The `.await` ensures we wait for the shutdown to complete.

    tokio::time::sleep(Duration::from_millis(1000)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests the execution of lifecycle handlers (`after_start`, `after_stop`).
///
/// **Scenario:**
/// 1. A `Counter` actor is created.
/// 2. It's configured to handle `Tally` messages by incrementing its count.
/// 3. An `after_stop` handler is added to assert the final count is 4.
/// 4. The `Counter` actor is started.
/// 5. Four `Tally` messages are sent.
/// 6. A `Messenger` actor is created with simple `after_start` and `after_stop` handlers (for logging).
/// 7. The `Messenger` actor is started.
/// 8. Both actors are stopped.
///
/// **Verification:**
/// - The `Counter` actor's `after_stop` handler asserts its `count` is 4.
/// - Tracing logs show the `Messenger` actor's lifecycle handlers being called.
#[acton_test]
async fn test_lifecycle_handlers() -> anyhow::Result<()> {
    // Initialize tracing for logging purposes
    initialize_tracing();

    // Launch the Acton runtime.
    let mut runtime: ActorRuntime = ActonApp::launch();

    // --- Counter Actor ---
    // Create a builder for the Counter actor.
    let mut counter_actor_builder = runtime.new_actor::<Counter>();
    counter_actor_builder
        // Handler for `Tally` messages.
        .mutate_on::<Tally>(|actor, _envelope| {
            info!("on tally");
            // Increment the internal counter.
            actor.model.count += 1;
            Reply::ready()
        })
        // Handler executed after the actor stops.
        .after_stop(|actor| {
            // Assert the final count after processing messages.
            assert_eq!(4, actor.model.count);
            trace!("on stopping");
            Reply::ready()
        });

    // Start the counter actor.
    let counter_handle = counter_actor_builder.start().await;

    // Send `Tally::AddCount` message four times.
    for _ in 0..4 {
        counter_handle.send(Tally {}).await;
    }

    // --- Messenger Actor ---
    // Create a builder for the Messenger actor.
    let mut messenger_actor_builder = runtime.new_actor::<Messenger>();
    messenger_actor_builder
        // Handler executed after the actor starts.
        .after_start(|_actor| {
            trace!("*");
            Reply::ready()
        })
        // Handler executed after the actor stops.
        .after_stop(|_actor| {
            trace!("*");
            Reply::ready()
        });

    // Start the messenger actor.
    let _messenger_handle = messenger_actor_builder.start().await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    // Stop both actors.
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests the creation and supervision of child actors.
///
/// **Scenario:**
/// 1. A parent `PoolItem` actor is created.
/// 2. A child `PoolItem` actor is created.
/// 3. The child is configured to handle `Ping` messages by incrementing its `receive_count`.
/// 4. An `after_stop` handler is added to the child to assert the final `receive_count` is 22.
/// 5. The parent actor is started.
/// 6. The parent actor is instructed to `supervise` the child actor (this also starts the child).
/// 7. The test verifies the parent now has 1 child in its `children` map.
/// 8. The test finds the child handle using `parent_handle.find_child()`.
/// 9. 22 `Ping` messages are sent directly to the child handle.
/// 10. The parent actor is stopped. Because the parent supervises the child, stopping the parent
///     will automatically trigger the stop sequence for the child as well.
///
/// **Verification:**
/// - Parent's child count is 1 after supervision.
/// - `find_child` successfully retrieves the child handle.
/// - Child's `after_stop` handler asserts `receive_count` is 22.
#[acton_test]
async fn test_child_actor() -> anyhow::Result<()> {
    // Initialize tracing for logging purposes
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch();

    // --- Parent Actor ---
    let parent_config = ActorConfig::new(
        Ern::with_root("test_child_actor_parent").unwrap(),
        None,
        None,
    )?;
    // Create the parent actor builder. No specific handlers needed for the parent itself in this test.
    let parent_actor_builder = runtime.new_actor_with_config::<PoolItem>(parent_config);

    // --- Child Actor ---
    let child_config = ActorConfig::new(
        Ern::with_root("test_child_actor_chile").unwrap(),
        None,
        None,
    )?;
    // Create the child actor builder.
    let mut child_actor_builder = runtime.new_actor_with_config::<PoolItem>(child_config);

    // Configure the child actor's behavior.
    child_actor_builder
        // Handler for `Ping` messages.
        .mutate_on::<Ping>(|actor, envelope| {
            match envelope.message() {
                Ping => {
                    // Increment the child's internal counter.
                    actor.model.receive_count += 1;
                }
            }
            Reply::ready()
        })
        // Handler executed after the child actor stops.
        .after_stop(|actor| {
            info!("Child processed {} PINGs", actor.model.receive_count);
            // Assert the final count after processing messages.
            assert_eq!(
                actor.model.receive_count, 22,
                "Child actor did not process the expected number of PINGs"
            );
            Reply::ready()
        });

    // Get the child's unique ID before it's moved during supervision.
    let child_id = child_actor_builder.id().clone();

    // Start the parent actor, obtaining its handle.
    let parent_handle = parent_actor_builder.start().await;

    // Tell the parent actor to supervise the child actor.
    // This transfers ownership of the child builder, starts the child actor,
    // and registers the child's handle within the parent's `children` map.
    parent_handle.supervise(child_actor_builder).await?;

    // Verify the parent now knows about the child.
    assert_eq!(
        parent_handle.children().len(),
        1,
        "Parent handle missing its child after supervision"
    );

    // Retrieve the child's handle from the parent using the child's ID.
    info!(child = &child_id.to_string(), "Searching all children for");
    let found_child_handle = parent_handle.find_child(&child_id);
    assert!(
        found_child_handle.is_some(),
        "Couldn't find child with id {child_id}"
    );
    // We get an Option<ActorHandle>, so unwrap it.
    let child_handle = found_child_handle.unwrap();

    // Send `Ping` messages directly to the child actor 22 times.
    for _ in 0..22 {
        trace!("Emitting PING");
        child_handle.send(Ping).await;
    }

    // Stop the parent actor. Because it supervises the child,
    // this will also initiate the stop sequence for the child actor.
    // The parent waits for the child to stop before it fully stops itself.
    trace!("Stopping parent actor (which should stop the child)");

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests finding a supervised child actor using its ID.
///
/// **Scenario:**
/// 1. A parent `PoolItem` actor is created and started.
/// 2. A child `PoolItem` actor is created.
/// 3. The parent supervises the child.
/// 4. The test verifies the parent has 1 child.
/// 5. The test attempts to find the child handle using `parent_handle.find_child()`.
/// 6. The parent is stopped.
///
/// **Verification:**
/// - Parent's child count is 1 after supervision.
/// - `find_child` successfully retrieves the child handle (the `is_some()` check passes).
#[acton_test]
async fn test_find_child_actor() -> anyhow::Result<()> {
    // Initialize tracing for logging purposes
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch();

    // --- Parent Actor ---
    let parent_actor_builder = runtime.new_actor::<PoolItem>();
    // Start the parent actor.
    let parent_handle = parent_actor_builder.start().await;

    // --- Child Actor ---
    let child_config = ActorConfig::new(
        Ern::with_root("test_find_child_actor_child").unwrap(),
        None,
        None,
    )?;
    let child_actor_builder = runtime.new_actor_with_config::<PoolItem>(child_config);
    // Get the child's ID before supervision.
    let child_id = child_actor_builder.id().clone();

    // Supervise the child actor.
    parent_handle.supervise(child_actor_builder).await?;

    // Verify parent knows about the child.
    assert_eq!(
        parent_handle.children().len(),
        1,
        "Parent handle missing its child after supervision"
    );

    // Find the child handle via the parent.
    info!(child = &child_id.to_string(), "Searching all children for");
    let found_child_handle = parent_handle.find_child(&child_id);
    assert!(
        found_child_handle.is_some(),
        "Couldn't find child with id {child_id}"
    );
    let _child_handle = found_child_handle.unwrap(); // We just need to confirm it was found.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the parent actor (and implicitly the child).
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests the ability of an actor to mutate its own internal state (`model`)
/// based on received messages.
///
/// **Scenario:**
/// 1. A `Comedian` actor is created.
/// 2. It's configured to handle `FunnyJoke` messages:
///    - Increment `jokes_told`.
///    - Send an `AudienceReactionMsg` back to itself (`Chuckle` or `Groan` based on the joke).
/// 3. It's configured to handle `AudienceReactionMsg` messages:
///    - Increment `funny` or `bombers` based on the reaction.
/// 4. An `after_stop` handler asserts the final counts for `jokes_told`, `funny`, and `bombers`.
/// 5. The actor is started.
/// 6. Two different `FunnyJoke` messages are sent.
/// 7. The actor is stopped.
///
/// **Verification:**
/// - The `after_stop` handler asserts `jokes_told` is 2, `funny` is 1, and `bombers` is 1.
#[acton_test]
async fn test_actor_mutation() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: ActorRuntime = ActonApp::launch();

    // Configure and create the Comedian actor builder.
    let actor_config =
        ActorConfig::new(Ern::with_root("test_actor_mutation").unwrap(), None, None)?;
    let mut comedian_actor_builder = runtime.new_actor_with_config::<Comedian>(actor_config);

    // Configure actor behavior.
    comedian_actor_builder
        // Handler for `FunnyJoke` messages.
        .mutate_on::<FunnyJoke>(|actor, envelope| {
            // Mutate state: increment jokes_told count.
            actor.model.jokes_told += 1;
            // Create a new envelope targeted back at this actor's address.
            let self_envelope = actor.new_envelope();
            // Clone the incoming message content to determine the reaction.
            let message = envelope.message().clone();
            // Return an Reply to send the reaction message.
            Reply::pending(async move {
                // Ensure the envelope was created successfully.
                if let Some(self_envelope) = self_envelope {
                    match message {
                        FunnyJoke::ChickenCrossesRoad => {
                            // Send a "Chuckle" reaction back to self.
                            let () = self_envelope.send(AudienceReactionMsg::Chuckle).await;
                        }
                        FunnyJoke::Pun => {
                            // Send a "Groan" reaction back to self.
                            let () = self_envelope.send(AudienceReactionMsg::Groan).await;
                        }
                    }
                }
            })
        })
        // Handler for `AudienceReactionMsg` (sent from the FunnyJoke handler above).
        .mutate_on::<AudienceReactionMsg>(|actor, envelope| {
            // Mutate state based on the reaction message content.
            match envelope.message() {
                AudienceReactionMsg::Chuckle => actor.model.funny += 1,
                AudienceReactionMsg::Groan => actor.model.bombers += 1,
            }
            Reply::ready()
        })
        // Handler executed after the actor stops.
        .after_stop(|actor| {
            info!(
                "Jokes told at {}: {}\tFunny: {}\tBombers: {}",
                actor.id(),
                actor.model.jokes_told,
                actor.model.funny,
                actor.model.bombers
            );
            // Assert the final state counts.
            assert_eq!(actor.model.jokes_told, 2);
            assert_eq!(actor.model.funny, 1);
            assert_eq!(actor.model.bombers, 1);
            Reply::ready()
        });

    // Start the actor.
    let comedian_handle = comedian_actor_builder.start().await;

    // Send the initial jokes to trigger the handlers.
    comedian_handle.send(FunnyJoke::ChickenCrossesRoad).await;
    comedian_handle.send(FunnyJoke::Pun).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the actor and wait for shutdown completion.
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests interaction between a parent and a child actor initiated from within the parent's message handler.
///
/// **Scenario:**
/// 1. A parent `Comedian` actor is created.
/// 2. A child `Counter` actor is created.
/// 3. The child is configured to handle `Ping` messages (logs receipt).
/// 4. The parent is configured to handle `FunnyJokeFor` messages:
///    - Extract the target child's ID from the message.
///    - Assert that the parent currently supervises one child.
///    - Find the child handle using `actor.handle().find_child()`.
///    - Assert that the child was found.
///    - Send a `Ping` message to the found child handle asynchronously.
/// 5. The parent supervises the child.
/// 6. The parent actor is started.
/// 7. The test asserts the parent handle shows 1 child *after* starting.
/// 8. A `FunnyJokeFor` message (containing the child's ID) is sent to the parent.
/// 9. The parent actor is stopped.
///
/// **Verification:**
/// - Assertions within the parent's `FunnyJokeFor` handler pass (child count is 1, child is found).
/// - Tracing logs show the child receiving the `Ping` message sent from the parent's handler.
/// - Assertion after parent start confirms child count is 1.
#[acton_test]
async fn test_child_count_in_reactor() -> anyhow::Result<()> {
    initialize_tracing();

    // Launch the Acton system and await its readiness
    let mut runtime: ActorRuntime = ActonApp::launch();

    // --- Parent Actor (Comedian) ---
    // Create the parent actor builder.
    let mut parent_actor_builder = runtime.new_actor::<Comedian>();

    // Configure the parent's message handler for `FunnyJokeFor`.
    parent_actor_builder.mutate_on::<FunnyJokeFor>(|actor, envelope| {
        // Extract the child ID from the incoming message.
        if let FunnyJokeFor::ChickenCrossesRoad(child_id) = envelope.message().clone() {
            info!("Got a funny joke for {}", &child_id);

            // **Inside the handler**, check if the parent knows about its children.
            // The `actor.handle()` provides access to the actor's context, including supervised children.
            assert_eq!(
                actor.handle().children().len(),
                1,
                "Parent actor missing any children in handler"
            );
            trace!("Parent actor has children: {:?}", actor.handle().children());

            // Attempt to find the specific child handle using the ID from the message.
            assert!(
                actor.handle().find_child(&child_id).is_some(),
                "No child found with ID {} in handler",
                &child_id
            );

            // Use the child handle if found, otherwise log an error.
            let maybe_child = actor.handle().find_child(&child_id);
            Reply::pending(async move {
                if let Some(child_handle) = maybe_child {
                    trace!("Pinging child {}", &child_id);
                    child_handle.send(Ping).await;
                } else {
                    // This case should not be hit based on the assertion above, but handle defensively.
                    tracing::error!("No child found with ID {}", &child_id);
                }
            })
        } else {
            // If the message wasn't the expected variant.
            Reply::ready()
        }
    });

    // --- Child Actor (Counter) ---
    // Configure and create the child actor builder.
    let child_config = ActorConfig::new(Ern::with_root("child").unwrap(), None, None)?;
    let mut child_actor_builder = runtime.new_actor_with_config::<Counter>(child_config);
    info!(
        "Created child actor builder with id: {}",
        child_actor_builder.id()
    );

    // Configure the child's message handler for `Ping`.
    child_actor_builder.mutate_on::<Ping>(|actor, _envelope| {
        info!("Child {} received Ping from parent actor", actor.id());
        Reply::ready()
    });

    // Get the child's ID before it's moved during supervision.
    let child_id = child_actor_builder.id().clone();

    // Supervise the child under the parent. Note: We use the *builder's* handle here,
    // as the parent actor hasn't been started yet. Supervision can happen during the Idle state.
    parent_actor_builder
        .handle()
        .supervise(child_actor_builder)
        .await?;
    // Verify the builder's handle reflects the supervised child immediately.
    assert_eq!(
        parent_actor_builder.handle().children().len(),
        1,
        "Parent builder missing its child after supervision"
    );

    // Start the parent actor. This also implicitly starts the already supervised child.
    let parent_handle = parent_actor_builder.start().await;

    // Assert that the *started* parent handle also reflects the child count correctly.
    assert_eq!(
        parent_handle.children().len(),
        1,
        "Started parent handle missing its child"
    );

    // Send the message to the parent, triggering the handler defined above.
    // Pass the child's ID so the parent knows which child to contact.
    parent_handle
        .send(FunnyJokeFor::ChickenCrossesRoad(child_id))
        .await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the parent actor (which will also stop the supervised child).
    runtime.shutdown_all().await?;
    Ok(())
}
