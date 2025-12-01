/*
 * Configuration Example for Acton Reactive
 *
 * This example demonstrates how Acton Reactive automatically loads configuration
 * from standard XDG locations and how you can verify that custom configuration
 * is being used.
 */

use acton_reactive::prelude::*;

/// Actor state for demonstrating configuration usage
#[acton_actor]
struct ConfigActor {
    /// Counter to track messages processed
    message_count: usize,
    /// Configuration value to verify custom settings
    custom_value: String,
}

/// Message to request actor configuration
#[acton_message]
struct GetConfig;

#[acton_main]
async fn main() {
    println!("=== Acton Reactive Configuration Example ===");
    println!("This example demonstrates automatic configuration loading.");
    println!("Configuration is loaded from: ~/.config/acton/config.toml");
    println!();

    // Launch the Acton runtime - configuration is loaded automatically
    let mut runtime = ActonApp::launch_async().await;

    // Create an actor with custom name to demonstrate configuration
    let mut actor_builder = runtime.new_actor::<ConfigActor>();

    // Set initial state
    actor_builder.model.custom_value = "configured_value".to_string();

    // Configure message handlers
    actor_builder
        .mutate_on::<GetConfig>(|actor, _envelope| {
            println!(
                "Current config: count={}, value={}",
                actor.model.message_count, actor.model.custom_value
            );
            Reply::ready()
        })
        .mutate_on::<Increment>(|actor, _| {
            actor.model.message_count += 1;
            println!("Processed message #{}", actor.model.message_count);
            Reply::ready()
        })
        .after_start(|_actor| {
            println!("Actor started with configuration loaded");
            println!("Check ~/.config/acton/config.toml to customize settings");
            Reply::ready()
        })
        .after_stop(|actor| {
            println!(
                "Actor stopped after processing {} messages",
                actor.model.message_count
            );
            Reply::ready()
        });

    // Start the actor
    let actor_handle = actor_builder.start().await;

    // Send some messages to demonstrate the actor is working
    actor_handle.send(Increment).await;
    actor_handle.send(Increment).await;
    actor_handle.send(Increment).await;

    // Request configuration information
    actor_handle.send(GetConfig).await;

    // Graceful shutdown
    runtime
        .shutdown_all()
        .await
        .expect("Failed to shut down system");

    println!();
    println!("=== Configuration Example Complete ===");
    println!("To customize this behavior:");
    println!("1. Copy examples/config.toml to ~/.config/acton/config.toml");
    println!("2. Modify the values to match your needs");
    println!("3. Run this example again to see the changes");
}

/// Simple increment message
#[acton_message]
struct Increment;
