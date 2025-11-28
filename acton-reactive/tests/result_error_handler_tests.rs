/*
 * Tests for result-based message handlers and agent-local error handler registration/dispatch.
 */

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Local test helpers - avoiding shared setup module to prevent dead_code warnings
// since each test binary compiles setup separately and not all types are used.

/// Counter agent state for testing error handlers
#[derive(Debug, Default, Clone)]
struct Counter {
    pub count: usize,
    pub errored: Option<bool>,
    pub errored2: Option<bool>,
}

#[derive(Clone, Debug)]
struct Ping;

#[derive(Clone, Debug)]
struct Tally;

#[acton_message]
struct Increment;

#[derive(Debug, Clone)]
struct TestErr;
#[derive(Debug, Clone)]
struct TestErr2;

impl std::fmt::Display for TestErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Deliberate test error")
    }
}
impl std::error::Error for TestErr {}

impl std::fmt::Display for TestErr2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Deliberate second test error")
    }
}
impl std::error::Error for TestErr2 {}

#[acton_test]
async fn test_result_and_error_handler_fires() -> anyhow::Result<()> {
    let mut runtime: AgentRuntime = ActonApp::launch();

    let agent_config = AgentConfig::new(Ern::with_root("error_handler_demo").unwrap(), None, None)?;

    let mut agent_builder = runtime.new_agent_with_config::<Counter>(agent_config).await;

    // Result-based handler for Ping
    agent_builder
        .mutate_on_fallible::<Ping, (), TestErr>(|_agent, _msg_ctx| Box::pin(async { Err(TestErr) }))
        .on_error::<Ping, TestErr>(|agent, _env, _err| {
            agent.model.errored = Some(true);
            AgentReply::immediate()
        });

    // Result-based handler for Tally triggers TestErr2
    agent_builder
        .mutate_on_fallible::<Tally, (), TestErr2>(|_agent, _msg_ctx| {
            println!("Ping handler for Tally fired!");
            Box::pin(async { Err(TestErr2) })
        })
        .on_error::<Tally, TestErr2>(|agent, _env, _err| {
            assert!(
                agent.model.errored2.is_none(),
                "TestErr2 error handler called more than once!"
            );
            agent.model.errored2 = Some(true);
            AgentReply::immediate()
        })
        .after_stop(|agent| {
            assert!(
                agent.model.errored.is_some(),
                "Error handler for TestErr was not called as expected (model.errored was not set)"
            );
            assert!(
                agent.model.errored2.is_some(),
                "Error handler for TestErr2 was not called as expected (model.errored2 was not set)"
            );
            AgentReply::immediate()
        });

    let agent_handle = agent_builder.start().await;
    agent_handle.send(Ping).await;
    agent_handle.send(Tally {}).await;
    agent_handle.stop().await?;

    Ok(())
}

#[acton_test]
async fn test_fallible_handler_returns_value() -> anyhow::Result<()> {
    let mut runtime: AgentRuntime = ActonApp::launch();

    let agent_config =
        AgentConfig::new(Ern::with_root("fallible_return_demo").unwrap(), None, None)?;

    let mut agent_builder = runtime.new_agent_with_config::<Counter>(agent_config).await;

    // A fallible handler that returns a value on success
    agent_builder
        .mutate_on_fallible::<Increment, usize, TestErr>(|agent, _msg_ctx| {
            agent.model.count += 1;
            let current_count = agent.model.count;
            Box::pin(async move { Ok(current_count) })
        })
        .on_error::<Increment, TestErr>(|_, _, _| {
            // This should not be called
            panic!("on_error should not be called in this test");
        })
        .after_stop(|agent| {
            assert_eq!(
                agent.model.count, 1,
                "The counter should have been incremented."
            );
            AgentReply::immediate()
        });

    let agent_handle = agent_builder.start().await;
    agent_handle.send(Increment).await;
    agent_handle.stop().await?;

    Ok(())
}
