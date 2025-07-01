/*
 * Tests for result-based message handlers and agent-local error handler registration/dispatch.
 */

use acton_reactive::prelude::*;
use acton_test::prelude::*;
mod setup;
use crate::setup::{
    actors::counter::Counter,
    initialize_tracing,
    messages::{Ping, Tally},
};

#[derive(Debug)]
struct TestErr;
struct TestErr2;

impl std::fmt::Display for TestErr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Deliberate test error")
    }
}
impl std::error::Error for TestErr {}

impl std::fmt::Display for TestErr2 {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Deliberate second test error")
    }
}
impl std::fmt::Debug for TestErr2 {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Deliberate second test error")
    }
}
impl std::error::Error for TestErr2 {}

#[acton_test]
async fn test_result_and_error_handler_fires() -> anyhow::Result<()> {
    initialize_tracing();
    let mut runtime: AgentRuntime = ActonApp::launch();

    let agent_config = AgentConfig::new(Ern::with_root("error_handler_demo").unwrap(), None, None)?;

    let mut agent_builder = runtime.new_agent_with_config::<Counter>(agent_config).await;

    // Result-based handler for Ping
    agent_builder
        .act_on_fallible::<Ping, (), TestErr>(|_agent, _msg_ctx| Box::pin(async { Err(TestErr) }))
        .on_error::<TestErr>(|agent, _env, _err| {
            agent.model.errored = true;
            AgentReply::immediate()
        });

    // Result-based handler for Tally triggers TestErr2
    agent_builder
        .act_on_fallible::<Tally, (), TestErr2>(|_agent, _msg_ctx| {
            println!("Ping handler for Tally fired!");
            Box::pin(async { Err(TestErr2) })
        })
        .on_error::<TestErr2>(|agent, _env, _err| {
            assert!(
                !agent.model.errored2,
                "TestErr2 error handler called more than once!"
            );
            agent.model.errored2 = true;
            AgentReply::immediate()
        })
        .after_stop(|agent| {
            assert!(
                agent.model.errored,
                "Error handler for TestErr was not called as expected (model.errored was not set)"
            );
            assert!(
                agent.model.errored2,
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
