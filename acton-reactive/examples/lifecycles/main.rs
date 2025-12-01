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
use tokio::time::{sleep, Duration};

/// Represents the state (model) for an actor that tracks a list of items.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
struct ItemTracker {
    /// A simple vector to store item names (as strings).
    items: Vec<String>,
}

/// Message to add a new item to the tracker's list.
#[acton_message]
struct AddItem(String);

/// Message to request the current list of items (demonstrates async handler).
#[acton_message]
struct GetItems;

#[acton_main]
async fn main() {
    // 1. Launch the Acton runtime.
    let mut runtime = ActonApp::launch_async().await;

    // 2. Create the actor builder for ItemTracker state.
    let mut tracker_actor_builder = runtime.new_actor::<ItemTracker>();

    // 3. Configure lifecycle hooks and message handlers.
    tracker_actor_builder
        // Hook executed *before* the actor's main task loop starts.
        .before_start(|_| {
            println!("Actor is preparing to track items... Here we go!");
            Reply::ready()
        })
        // Hook executed *after* the actor's main task loop has started.
        .after_start(|_| {
            println!("Actor is now tracking items!");
            Reply::ready()
        })
        // Handler for `AddItem` messages.
        .mutate_on::<AddItem>(|actor, envelope| {
            let item = &envelope.message().0;
            println!("Adding item: {item}");
            // Mutate the actor's internal state.
            actor.model.items.push(item.clone());
            Reply::ready()
        })
        // Handler for `GetItems` messages.
        .mutate_on::<GetItems>(|actor, _| {
            println!("Fetching items... please wait!");
            // Clone the items list to move it into the async block.
            let items = actor.model.items.clone();
            // Use `from_async` to perform work asynchronously.
            Reply::pending(async move {
                // Simulate a delay (e.g., fetching from a database).
                sleep(Duration::from_secs(2)).await;
                println!("Current items: {items:?}");
            })
        })
        // Hook executed *before* the actor starts its shutdown process
        // (after receiving a stop signal but before stopping children).
        .before_stop(|_| {
            println!("Actor is stopping... finishing up!");
            Reply::ready()
        })
        // Hook executed *after* the actor's task has fully stopped
        // and all children (if any) have stopped.
        .after_stop(|actor| {
            println!("Actor stopped! Final items: {:?}", actor.model.items);
            // Assert the final state.
            debug_assert_eq!(actor.model.items, vec!["Apple", "Banana", "Cherry"]);
            Reply::ready()
        });

    // 4. Start the actor. Lifecycle: before_start -> after_start.
    let tracker_handle = tracker_actor_builder.start().await;

    // 5. Send messages to the actor.
    tracker_handle.send(AddItem("Apple".to_string())).await;
    tracker_handle.send(AddItem("Banana".to_string())).await;
    tracker_handle.send(AddItem("Cherry".to_string())).await;

    // Send GetItems, which will print after a delay.
    tracker_handle.send(GetItems).await;

    // Allow time for GetItems async handler to complete before shutdown.
    sleep(Duration::from_secs(3)).await;

    // 6. Shut down the runtime. Lifecycle: before_stop -> after_stop.
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shut down system");
}
