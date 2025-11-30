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

use acton_reactive::prelude::*;

/// Defines the internal state (the "model") for our basic actor.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
struct BasicActorState {
    /// Simple counter state modified by message handlers.
    some_state: usize,
}

/// A message used to initiate interaction with the actor.
// The `#[acton_message]` macro derives `Clone` and `Debug`.
#[acton_message]
struct PingMsg;

/// A reply message sent back by the actor after receiving `PingMsg`.
#[acton_message]
struct PongMsg;

/// A final message sent by the actor before stopping.
#[acton_message]
struct BuhByeMsg;

#[tokio::main]
async fn main() {
    // 1. Launch the Acton runtime environment.
    let mut runtime = ActonApp::launch();

    // 2. Create an actor builder.
    //    `new_actor` takes the *state type* (model) as a generic parameter.
    //    It returns a builder (`ManagedActor` in the `Idle` state) which we use to configure the actor.
    let mut actor_builder = runtime.new_actor::<BasicActorState>();

    // 3. Configure the actor's behavior by defining message handlers using `mutate_on`.
    actor_builder
        // Define a handler for `PingMsg`.
        // The closure receives the `ManagedActor` (giving access to `model` and `handle`)
        // and the incoming message `envelope`.
        .mutate_on::<PingMsg>(|actor, envelope| {
            println!("Pinged. You can mutate me!");
            // Access and modify the actor's internal state (`model`).
            actor.model.some_state += 1;

            // Get an envelope pre-addressed to reply to the sender of the incoming `PingMsg`.
            let reply_envelope = envelope.reply_envelope();

            // Handlers must return a future (specifically `ActorReply`, which wraps a `Pin<Box<dyn Future>>`).
            // This allows handlers to perform asynchronous operations.
            Box::pin(async move {
                // Send the `PongMsg` back to the original sender.
                reply_envelope.send(PongMsg).await;
            })
        })
        // Define a handler for `PongMsg`.
        .mutate_on::<PongMsg>(|actor, _envelope| {
            println!("I got ponged!");
            actor.model.some_state += 1;

            // Get a clone of the actor's own handle to send a message to itself.
            // Cloning is necessary to move the handle into the async block.
            let self_handle = actor.handle().clone();

            // `ActorReply::from_async` is a helper to wrap a future in the required type.
            ActorReply::from_async(async move {
                // Send `BuhByeMsg` to self.
                self_handle.send(BuhByeMsg).await;
            })
        })
        // Define a handler for `BuhByeMsg`.
        .mutate_on::<BuhByeMsg>(|_actor, _envelope| {
            println!("Thanks for all the fish! Buh Bye!");
            // If a handler doesn't need to perform async work or reply,
            // `ActorReply::immediate()` signifies immediate completion.
            ActorReply::immediate()
        })
        // Define a callback that runs after the actor stops.
        .after_stop(|actor| {
            println!("Actor stopped with state value: {}", actor.model.some_state);
            // Assert the final state after all messages are processed.
            debug_assert_eq!(actor.model.some_state, 2);
            ActorReply::immediate()
        });

    // 4. Start the actor.
    //    This transitions the actor to the `Started` state, spawns its task,
    //    and returns an `ActorHandle` for interaction.
    let actor_handle = actor_builder.start().await;

    // 5. Send the initial `PingMsg` to the running actor using its handle.
    actor_handle.send(PingMsg).await;

    // 6. Shut down the runtime.
    //    This gracefully stops all actors managed by the runtime, allowing them
    //    to finish processing messages and run their `after_stop` handlers.
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shut down system");
}
