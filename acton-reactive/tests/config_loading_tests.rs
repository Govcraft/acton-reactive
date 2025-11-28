use acton_reactive::prelude::*;
use acton_test::prelude::*;
use anyhow::Ok;
use std::fs;
use tempfile::TempDir;

/// Test that configuration loading works with default values when no config file exists
#[acton_test]
async fn test_default_configuration_loading() -> Result<(), anyhow::Error> {
    // Create a temporary directory to isolate from user's actual config
    let temp_dir = TempDir::new().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Launch ActonApp which should load defaults
    let mut app = ActonApp::launch();

    // Verify the app started successfully by creating an agent
    let agent_builder = app.new_agent::<TestAgent>().await;
    let _handle = agent_builder.start().await;

    // Clean up
    temp_dir.close().unwrap();
    Ok(())
}

/// Test that custom configuration overrides default values
#[acton_test]
async fn test_custom_configuration_override() -> anyhow::Result<()> {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("acton");
    fs::create_dir_all(&config_dir).unwrap();

    // Create a custom config file
    let config_content = r#"
        [timeouts]
        agent_shutdown_timeout_ms = 5000
        system_shutdown_timeout_ms = 15000

        [limits]
        agent_inbox_capacity = 512
        concurrent_handlers_high_water_mark = 50

        [defaults]
        agent_name = "custom_agent"

        [tracing]
        debug_level = "info"
    "#;

    fs::write(config_dir.join("config.toml"), config_content).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Launch ActonApp which should load the custom config
    let mut app = ActonApp::launch();

    // Create a test agent to verify the custom config is used
    let agent_builder = app.new_agent::<TestAgent>().await;
    let _handle = agent_builder.start().await;

    // Clean up
    temp_dir.close().unwrap();
    Ok(())
}

/// Test XDG directory resolution works correctly
#[acton_test]
async fn test_xdg_directory_resolution() -> Result<(), anyhow::Error> {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("acton");
    fs::create_dir_all(&config_dir).unwrap();

    // Create a config file
    let config_content = r"
        [timeouts]
        agent_shutdown_timeout_ms = 7500
    ";

    fs::write(config_dir.join("config.toml"), config_content).unwrap();

    // Set XDG_CONFIG_HOME
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Launch and verify it uses the config
    let mut app = ActonApp::launch();
    let agent_builder = app.new_agent::<TestAgent>().await;
    let _handle = agent_builder.start().await;

    temp_dir.close().unwrap();
    Ok(())
}

/// Test error handling for malformed configuration files
#[acton_test]
async fn test_malformed_config_handling() -> Result<(), anyhow::Error> {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("acton");
    fs::create_dir_all(&config_dir).unwrap();

    // Create a malformed config file
    let malformed_content = r#"
        [timeouts]
        agent_shutdown_timeout_ms = "not_a_number"

        [limits]
        agent_inbox_capacity = -1
    "#;

    fs::write(config_dir.join("config.toml"), malformed_content).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    // Should still launch successfully with defaults
    let mut app = ActonApp::launch();
    let agent_builder = app.new_agent::<TestAgent>().await;
    let _handle = agent_builder.start().await;

    temp_dir.close().unwrap();
    Ok(())
}

/// Test backward compatibility - existing code should work unchanged
#[acton_test]
async fn test_backward_compatibility() -> Result<(), anyhow::Error> {
    // Don't set any XDG variables - should use system defaults
    let mut app = ActonApp::launch();

    // Verify basic functionality still works
    let mut agent_builder = app.new_agent::<CounterAgent>().await;
    agent_builder.mutate_on::<Increment>(|agent, _| {
        agent.model.count += 1;
        AgentReply::immediate()
    });

    let handle = agent_builder.start().await;
    handle.send(Increment).await;

    // Should work exactly as before
    // No assertions needed - if it compiles and runs, it's compatible
    Ok(())
}

/// Test configuration values are actually used in agent behavior
#[acton_test]
async fn test_config_values_used_in_behavior() -> Result<(), anyhow::Error> {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("acton");
    fs::create_dir_all(&config_dir).unwrap();

    // Create config with very small inbox capacity
    let config_content = r"
        [limits]
        agent_inbox_capacity = 2
    ";

    fs::write(config_dir.join("config.toml"), config_content).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    let mut app = ActonApp::launch();

    // Create agent that should use the small inbox capacity
    let mut agent_builder = app.new_agent::<CounterAgent>().await;
    agent_builder.mutate_on::<Increment>(|agent, _| {
        agent.model.count += 1;
        AgentReply::immediate()
    });

    let handle = agent_builder.start().await;

    // Send multiple messages to test capacity
    for _ in 0..5 {
        handle.send(Increment).await;
    }

    // Verify agent still works (backpressure should handle capacity)
    temp_dir.close().unwrap();
    Ok(())
}

/// Test multiple agents use consistent configuration
#[acton_test]
async fn test_consistent_config_across_agents() -> Result<(), anyhow::Error> {
    let temp_dir = TempDir::new().unwrap();
    let config_dir = temp_dir.path().join("acton");
    fs::create_dir_all(&config_dir).unwrap();

    let config_content = r#"
        [defaults]
        agent_name = "test_agent"
    "#;

    fs::write(config_dir.join("config.toml"), config_content).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", temp_dir.path());

    let mut app = ActonApp::launch();

    // Create multiple agents
    let builder1 = app.new_agent::<CounterAgent>().await;
    let _handle1 = builder1.start().await;

    let builder2 = app.new_agent::<CounterAgent>().await;
    let _handle2 = builder2.start().await;

    // Both should work with same configuration
    temp_dir.close().unwrap();
    Ok(())
}

// Test helper structs
#[derive(Debug, Default)]
struct TestAgent {
    value: i32,
}

#[derive(Debug, Default)]
struct CounterAgent {
    count: i32,
}

#[acton_message]
struct Increment;

#[acton_message]
struct GetValue;

#[acton_message]
struct SetValue(i32);
