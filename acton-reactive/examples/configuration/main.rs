/*
 * Configuration Example for Acton Reactive
 *
 * This example demonstrates how Acton Reactive automatically loads configuration
 * from standard XDG locations and how you can verify that custom configuration
 * is being used.
 */

use acton_reactive::prelude::*;
use acton_macro::acton_actor;

/// Agent state for demonstrating configuration usage
#[acton_actor]
struct ConfigAgent {
    /// Counter to track messages processed
    message_count: usize,
    /// Configuration value to verify custom settings
    custom_value: String,
}

/// Message to request agent configuration
#[derive(Debug, Clone)]
struct GetConfig;

/// Response containing agent configuration
#[derive(Debug, Clone)]
struct ConfigResponse {
    message_count: usize,
    custom_value: String,
    agent_name: String,
}

#[tokio::main]
async fn main() {
    println!("=== Acton Reactive Configuration Example ===");
    println!("This example demonstrates automatic configuration loading.");
    println!("Configuration is loaded from: ~/.config/acton/config.toml");
    println!();

    // Launch the Acton runtime - configuration is loaded automatically
    let mut runtime = ActonApp::launch();

    // Create an agent with custom name to demonstrate configuration
    let mut agent_builder = runtime.new_agent::<ConfigAgent>().await;

    // Set initial state
    agent_builder.model.custom_value = "configured_value".to_string();

    // Configure message handlers
    agent_builder
        .mutate_on::<GetConfig>(|agent, envelope| {
            let response = ConfigResponse {
                message_count: agent.model.message_count,
                custom_value: agent.model.custom_value.clone(),
                agent_name: "config_agent".to_string(),
            };

            let reply_envelope = envelope.reply_envelope();
            AgentReply::from_async(async move {
                reply_envelope.send(response).await;
            })
        })
        .mutate_on::<Increment>(|agent, _| {
            agent.model.message_count += 1;
            println!("Processed message #{}", agent.model.message_count);
            AgentReply::immediate()
        })
        .after_start(|agent| {
            println!("Agent started with configuration loaded");
            println!("Check ~/.config/acton/config.toml to customize settings");
            AgentReply::immediate()
        })
        .after_stop(|agent| {
            println!("Agent stopped after processing {} messages", agent.model.message_count);
            AgentReply::immediate()
        });

    // Start the agent
    let agent_handle = agent_builder.start().await;

    // Send some messages to demonstrate the agent is working
    agent_handle.send(Increment).await;
    agent_handle.send(Increment).await;
    agent_handle.send(Increment).await;

    // Request configuration information
    let config = agent_handle.send(GetConfig).await;
    println!("Agent configuration: {config:?}");

    // Graceful shutdown
    runtime.shutdown_all().await.expect("Failed to shut down system");

    println!();
    println!("=== Configuration Example Complete ===");
    println!("To customize this behavior:");
    println!("1. Copy examples/config.toml to ~/.config/acton/config.toml");
    println!("2. Modify the values to match your needs");
    println!("3. Run this example again to see the changes");
}

/// Simple increment message
#[derive(Debug, Clone)]
struct Increment;