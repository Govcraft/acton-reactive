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

/// Tests the ability of an agent to handle asynchronous operations within its message handlers (`act_on`).
///
/// **Scenario:**
/// 1. A `Comedian` agent is created.
/// 2. It's configured to handle `FunnyJoke` messages:
///    - Increment its `jokes_told` counter.
///    - Asynchronously send a `Ping` message back to itself.
/// 3. It's configured to handle `AudienceReactionMsg` messages:
///    - Increment `funny` or `bombers` counters based on the reaction.
/// 4. It's configured to handle `Ping` messages (just logs).
/// 5. An `after_stop` handler is added to assert the final state (`jokes_told` count).
/// 6. The agent is started.
/// 7. Two `FunnyJoke` messages are sent to the agent.
/// 8. The agent is stopped.
///
/// **Verification:**
/// - The `after_stop` handler asserts that `jokes_told` is 2.
/// - The internal `Ping` messages are sent and handled (verified by tracing logs).
#[acton_test]
async fn test_async_reactor() -> anyhow::Result<()> {
    initialize_tracing();

    // Launch the Acton runtime environment. This provides the necessary infrastructure
    // for creating and managing agents.
    let mut runtime: AgentRuntime = ActonApp::launch();

    // Configure the agent's identity (ERN) and relationship (no parent, no broker needed here).
    let agent_config = AgentConfig::new(Ern::with_root("improve_show").unwrap(), None, None)?;
    // Create an agent builder for the `Comedian` state (`model`).
    // The builder is in an `Idle` state, ready for configuration.
    let mut comedian_agent_builder = runtime
        .new_agent_with_config::<Comedian>(agent_config)
        .await;

    // Configure the agent's behavior by defining message handlers using `act_on`.
    comedian_agent_builder
        // Define a handler for messages of type `FunnyJoke`.
        // The closure receives the `ManagedAgent` (giving access to `model` and `handle`)
        // and the incoming message `envelope`.
        .act_on::<FunnyJoke>(|agent, _envelope| {
            // Mutate the agent's internal state (`model`).
            agent.model.jokes_told += 1;
            // Clone the agent's handle. Handles are cheap to clone and allow interaction
            // with the agent (like sending messages) from outside the agent's task.
            let agent_handle = agent.handle().clone();
            // Return a pinned future (`Pin<Box<dyn Future>>`). This allows the handler
            // to perform asynchronous operations after the initial synchronous part.
            Box::pin(async move {
                trace!("emitting async");
                // Send a `Ping` message back to the agent itself asynchronously.
                agent_handle.send(Ping).await;
            })
        })
        // Define a handler for `AudienceReactionMsg` messages.
        .act_on::<AudienceReactionMsg>(|agent, envelope| {
            trace!("Received Audience Reaction");
            // Access the message content from the envelope.
            match envelope.message() {
                AudienceReactionMsg::Chuckle => agent.model.funny += 1,
                AudienceReactionMsg::Groan => agent.model.bombers += 1,
            };
            // This handler completes synchronously, so we return `AgentReply::immediate()`.
            AgentReply::immediate()
        })
        // Define a handler for `Ping` messages (sent from the FunnyJoke handler).
        .act_on::<Ping>(|_agent, _envelope| {
            trace!("PING");
            AgentReply::immediate()
        })
        // Define a callback function to execute *after* the agent has stopped processing messages
        // and its internal task has finished.
        .after_stop(|agent| {
            info!(
                "Jokes told at {}: {}\tFunny: {}\tBombers: {}",
                agent.id(),             // Access agent's unique ID.
                agent.model.jokes_told, // Access final state.
                agent.model.funny,
                agent.model.bombers
            );
            // Assert the final state after the agent has run.
            assert_eq!(agent.model.jokes_told, 2);
            AgentReply::immediate()
        });

    // Transition the agent from `Idle` to `Started`. This spawns the agent's main task loop
    // and returns an `AgentHandle` for interaction.
    let comedian_handle = comedian_agent_builder.start().await;

    // Send messages to the now running agent using its handle.
    comedian_handle.send(FunnyJoke::ChickenCrossesRoad).await;
    comedian_handle.send(FunnyJoke::Pun).await;

    // Initiate the agent's shutdown sequence. This sends a `Terminate` signal.
    // The agent will finish processing any messages currently in its inbox,
    // then execute its `after_stop` handler before fully terminating.
    // The `.await` ensures we wait for the shutdown to complete.

    tokio::time::sleep(Duration::from_millis(1000)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests the execution of lifecycle handlers (`after_start`, `after_stop`).
///
/// **Scenario:**
/// 1. A `Counter` agent is created.
/// 2. It's configured to handle `Tally` messages by incrementing its count.
/// 3. An `after_stop` handler is added to assert the final count is 4.
/// 4. The `Counter` agent is started.
/// 5. Four `Tally` messages are sent.
/// 6. A `Messenger` agent is created with simple `after_start` and `after_stop` handlers (for logging).
/// 7. The `Messenger` agent is started.
/// 8. Both agents are stopped.
///
/// **Verification:**
/// - The `Counter` agent's `after_stop` handler asserts its `count` is 4.
/// - Tracing logs show the `Messenger` agent's lifecycle handlers being called.
#[acton_test]
async fn test_lifecycle_handlers() -> anyhow::Result<()> {
    // Initialize tracing for logging purposes
    initialize_tracing();

    // Launch the Acton runtime.
    let mut runtime: AgentRuntime = ActonApp::launch();

    // --- Counter Agent ---
    // Create a builder for the Counter agent.
    let mut counter_agent_builder = runtime.new_agent::<Counter>().await;
    counter_agent_builder
        // Handler for `Tally` messages.
        .act_on::<Tally>(|agent, _envelope| {
            info!("on tally");
            // Increment the internal counter.
            agent.model.count += 1;
            AgentReply::immediate()
        })
        // Handler executed after the agent stops.
        .after_stop(|agent| {
            // Assert the final count after processing messages.
            assert_eq!(4, agent.model.count);
            trace!("on stopping");
            AgentReply::immediate()
        });

    // Start the counter agent.
    let counter_handle = counter_agent_builder.start().await;

    // Send `Tally::AddCount` message four times.
    for _ in 0..4 {
        counter_handle.send(Tally {}).await;
    }

    // --- Messenger Agent ---
    // Create a builder for the Messenger agent.
    let mut messenger_agent_builder = runtime.new_agent::<Messenger>().await;
    messenger_agent_builder
        // Handler executed after the agent starts.
        .after_start(|_agent| {
            trace!("*");
            AgentReply::immediate()
        })
        // Handler executed after the agent stops.
        .after_stop(|_agent| {
            trace!("*");
            AgentReply::immediate()
        });

    // Start the messenger agent.
    let messenger_handle = messenger_agent_builder.start().await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    // Stop both agents.
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests the creation and supervision of child agents.
///
/// **Scenario:**
/// 1. A parent `PoolItem` agent is created.
/// 2. A child `PoolItem` agent is created.
/// 3. The child is configured to handle `Ping` messages by incrementing its `receive_count`.
/// 4. An `after_stop` handler is added to the child to assert the final `receive_count` is 22.
/// 5. The parent agent is started.
/// 6. The parent agent is instructed to `supervise` the child agent (this also starts the child).
/// 7. The test verifies the parent now has 1 child in its `children` map.
/// 8. The test finds the child handle using `parent_handle.find_child()`.
/// 9. 22 `Ping` messages are sent directly to the child handle.
/// 10. The parent agent is stopped. Because the parent supervises the child, stopping the parent
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
    let mut runtime: AgentRuntime = ActonApp::launch();

    // --- Parent Agent ---
    let parent_config = AgentConfig::new(
        Ern::with_root("test_child_actor_parent").unwrap(),
        None,
        None,
    )?;
    // Create the parent agent builder. No specific handlers needed for the parent itself in this test.
    let parent_agent_builder = runtime
        .new_agent_with_config::<PoolItem>(parent_config)
        .await;

    // --- Child Agent ---
    let child_config = AgentConfig::new(
        Ern::with_root("test_child_actor_chile").unwrap(),
        None,
        None,
    )?;
    // Create the child agent builder.
    let mut child_agent_builder = runtime
        .new_agent_with_config::<PoolItem>(child_config)
        .await;

    // Configure the child agent's behavior.
    child_agent_builder
        // Handler for `Ping` messages.
        .act_on::<Ping>(|agent, envelope| {
            match envelope.message() {
                Ping => {
                    // Increment the child's internal counter.
                    agent.model.receive_count += 1;
                }
            };
            AgentReply::immediate()
        })
        // Handler executed after the child agent stops.
        .after_stop(|agent| {
            info!("Child processed {} PINGs", agent.model.receive_count);
            // Assert the final count after processing messages.
            assert_eq!(
                agent.model.receive_count, 22,
                "Child actor did not process the expected number of PINGs"
            );
            AgentReply::immediate()
        });

    // Get the child's unique ID before it's moved during supervision.
    let child_id = child_agent_builder.id().clone();

    // Start the parent agent, obtaining its handle.
    let parent_handle = parent_agent_builder.start().await;

    // Tell the parent agent to supervise the child agent.
    // This transfers ownership of the child builder, starts the child agent,
    // and registers the child's handle within the parent's `children` map.
    parent_handle.supervise(child_agent_builder).await?;

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
        "Couldn't find child with id {}",
        child_id
    );
    // We get an Option<AgentHandle>, so unwrap it.
    let child_handle = found_child_handle.unwrap();

    // Send `Ping` messages directly to the child agent 22 times.
    for _ in 0..22 {
        trace!("Emitting PING");
        child_handle.send(Ping).await;
    }

    // Stop the parent agent. Because it supervises the child,
    // this will also initiate the stop sequence for the child agent.
    // The parent waits for the child to stop before it fully stops itself.
    trace!("Stopping parent agent (which should stop the child)");

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests finding a supervised child agent using its ID.
///
/// **Scenario:**
/// 1. A parent `PoolItem` agent is created and started.
/// 2. A child `PoolItem` agent is created.
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
    let mut runtime: AgentRuntime = ActonApp::launch();

    // --- Parent Agent ---
    let parent_agent_builder = runtime.new_agent::<PoolItem>().await;
    // Start the parent agent.
    let parent_handle = parent_agent_builder.start().await;

    // --- Child Agent ---
    let child_config = AgentConfig::new(
        Ern::with_root("test_find_child_actor_child").unwrap(),
        None,
        None,
    )?;
    let child_agent_builder = runtime
        .new_agent_with_config::<PoolItem>(child_config)
        .await;
    // Get the child's ID before supervision.
    let child_id = child_agent_builder.id().clone();

    // Supervise the child agent.
    parent_handle.supervise(child_agent_builder).await?;

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
        "Couldn't find child with id {}",
        child_id
    );
    let _child_handle = found_child_handle.unwrap(); // We just need to confirm it was found.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the parent agent (and implicitly the child).
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests the ability of an agent to mutate its own internal state (`model`)
/// based on received messages.
///
/// **Scenario:**
/// 1. A `Comedian` agent is created.
/// 2. It's configured to handle `FunnyJoke` messages:
///    - Increment `jokes_told`.
///    - Send an `AudienceReactionMsg` back to itself (`Chuckle` or `Groan` based on the joke).
/// 3. It's configured to handle `AudienceReactionMsg` messages:
///    - Increment `funny` or `bombers` based on the reaction.
/// 4. An `after_stop` handler asserts the final counts for `jokes_told`, `funny`, and `bombers`.
/// 5. The agent is started.
/// 6. Two different `FunnyJoke` messages are sent.
/// 7. The agent is stopped.
///
/// **Verification:**
/// - The `after_stop` handler asserts `jokes_told` is 2, `funny` is 1, and `bombers` is 1.
#[acton_test]
async fn test_actor_mutation() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();

    // Configure and create the Comedian agent builder.
    let agent_config =
        AgentConfig::new(Ern::with_root("test_actor_mutation").unwrap(), None, None)?;
    let mut comedian_agent_builder = runtime
        .new_agent_with_config::<Comedian>(agent_config)
        .await;

    // Configure agent behavior.
    comedian_agent_builder
        // Handler for `FunnyJoke` messages.
        .act_on::<FunnyJoke>(|agent, envelope| {
            // Mutate state: increment jokes_told count.
            agent.model.jokes_told += 1;
            // Create a new envelope targeted back at this agent's address.
            let self_envelope = agent.new_envelope();
            // Clone the incoming message content to determine the reaction.
            let message = envelope.message().clone();
            // Return an async block to send the reaction message.
            Box::pin(async move {
                // Ensure the envelope was created successfully.
                if let Some(self_envelope) = self_envelope {
                    match message {
                        FunnyJoke::ChickenCrossesRoad => {
                            // Send a "Chuckle" reaction back to self.
                            let _ = self_envelope.send(AudienceReactionMsg::Chuckle).await;
                        }
                        FunnyJoke::Pun => {
                            // Send a "Groan" reaction back to self.
                            let _ = self_envelope.send(AudienceReactionMsg::Groan).await;
                        }
                    }
                }
            })
        })
        // Handler for `AudienceReactionMsg` (sent from the FunnyJoke handler above).
        .act_on::<AudienceReactionMsg>(|agent, envelope| {
            // Mutate state based on the reaction message content.
            match envelope.message() {
                AudienceReactionMsg::Chuckle => agent.model.funny += 1,
                AudienceReactionMsg::Groan => agent.model.bombers += 1,
            };
            AgentReply::immediate()
        })
        // Handler executed after the agent stops.
        .after_stop(|agent| {
            info!(
                "Jokes told at {}: {}\tFunny: {}\tBombers: {}",
                agent.id(),
                agent.model.jokes_told,
                agent.model.funny,
                agent.model.bombers
            );
            // Assert the final state counts.
            assert_eq!(agent.model.jokes_told, 2);
            assert_eq!(agent.model.funny, 1);
            assert_eq!(agent.model.bombers, 1);
            AgentReply::immediate()
        });

    // Start the agent.
    let comedian_handle = comedian_agent_builder.start().await;

    // Send the initial jokes to trigger the handlers.
    comedian_handle.send(FunnyJoke::ChickenCrossesRoad).await;
    comedian_handle.send(FunnyJoke::Pun).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stop the agent and wait for shutdown completion.
    runtime.shutdown_all().await?;

    Ok(())
}

/// Tests interaction between a parent and a child agent initiated from within the parent's message handler.
///
/// **Scenario:**
/// 1. A parent `Comedian` agent is created.
/// 2. A child `Counter` agent is created.
/// 3. The child is configured to handle `Ping` messages (logs receipt).
/// 4. The parent is configured to handle `FunnyJokeFor` messages:
///    - Extract the target child's ID from the message.
///    - Assert that the parent currently supervises one child.
///    - Find the child handle using `agent.handle().find_child()`.
///    - Assert that the child was found.
///    - Send a `Ping` message to the found child handle asynchronously.
/// 5. The parent supervises the child.
/// 6. The parent agent is started.
/// 7. The test asserts the parent handle shows 1 child *after* starting.
/// 8. A `FunnyJokeFor` message (containing the child's ID) is sent to the parent.
/// 9. The parent agent is stopped.
///
/// **Verification:**
/// - Assertions within the parent's `FunnyJokeFor` handler pass (child count is 1, child is found).
/// - Tracing logs show the child receiving the `Ping` message sent from the parent's handler.
/// - Assertion after parent start confirms child count is 1.
#[acton_test]
async fn test_child_count_in_reactor() -> anyhow::Result<()> {
    initialize_tracing();

    // Launch the Acton system and await its readiness
    let mut runtime: AgentRuntime = ActonApp::launch();

    // --- Parent Agent (Comedian) ---
    // Create the parent agent builder.
    let mut parent_agent_builder = runtime.new_agent::<Comedian>().await;

    // Configure the parent's message handler for `FunnyJokeFor`.
    parent_agent_builder.act_on::<FunnyJokeFor>(|agent, envelope| {
        // Extract the child ID from the incoming message.
        if let FunnyJokeFor::ChickenCrossesRoad(child_id) = envelope.message().clone() {
            info!("Got a funny joke for {}", &child_id);

            // **Inside the handler**, check if the parent knows about its children.
            // The `agent.handle()` provides access to the agent's context, including supervised children.
            assert_eq!(
                agent.handle().children().len(),
                1,
                "Parent agent missing any children in handler"
            );
            trace!("Parent agent has children: {:?}", agent.handle().children());

            // Attempt to find the specific child handle using the ID from the message.
            assert!(
                agent.handle().find_child(&child_id).is_some(),
                "No child found with ID {} in handler",
                &child_id
            );

            // If the child handle is found...
            if let Some(child_handle) = agent.handle().find_child(&child_id) {
                trace!("Pinging child {}", &child_id);
                // Clone the handle for use in the async block.
                let child_handle = child_handle.clone();
                // Return an async block to send the `Ping` message to the child.
                AgentReply::from_async(async move { child_handle.send(Ping).await })
            } else {
                // This case should not be hit based on the assertion above, but handle defensively.
                tracing::error!("No child found with ID {}", &child_id);
                AgentReply::immediate()
            }
        } else {
            // If the message wasn't the expected variant.
            AgentReply::immediate()
        }
    });

    // --- Child Agent (Counter) ---
    // Configure and create the child agent builder.
    let child_config = AgentConfig::new(Ern::with_root("child").unwrap(), None, None)?;
    let mut child_agent_builder = runtime.new_agent_with_config::<Counter>(child_config).await;
    info!(
        "Created child agent builder with id: {}",
        child_agent_builder.id()
    );

    // Configure the child's message handler for `Ping`.
    child_agent_builder.act_on::<Ping>(|agent, _envelope| {
        info!("Child {} received Ping from parent agent", agent.id());
        AgentReply::immediate()
    });

    // Get the child's ID before it's moved during supervision.
    let child_id = child_agent_builder.id().clone();

    // Supervise the child under the parent. Note: We use the *builder's* handle here,
    // as the parent agent hasn't been started yet. Supervision can happen during the Idle state.
    parent_agent_builder
        .handle()
        .supervise(child_agent_builder)
        .await?;
    // Verify the builder's handle reflects the supervised child immediately.
    assert_eq!(
        parent_agent_builder.handle().children().len(),
        1,
        "Parent builder missing its child after supervision"
    );

    // Start the parent agent. This also implicitly starts the already supervised child.
    let parent_handle = parent_agent_builder.start().await;

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

    // Stop the parent agent (which will also stop the supervised child).
    runtime.shutdown_all().await?;
    Ok(())
}
