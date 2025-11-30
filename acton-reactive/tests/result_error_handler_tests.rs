/*
 * Tests for result-based message handlers and actor-local error handler registration/dispatch.
 */

use acton_reactive::prelude::*;
use acton_test::prelude::*;

// Local test helpers - avoiding shared setup module to prevent dead_code warnings
// since each test binary compiles setup separately and not all types are used.

/// Counter actor state for testing error handlers
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
    let mut runtime: ActorRuntime = ActonApp::launch();

    let actor_config = ActorConfig::new(Ern::with_root("error_handler_demo").unwrap(), None, None)?;

    let mut actor_builder = runtime.new_actor_with_config::<Counter>(actor_config);

    // Result-based handler for Ping
    actor_builder
        .try_mutate_on::<Ping, (), TestErr>(|_actor, _msg_ctx| Box::pin(async { Err(TestErr) }))
        .on_error::<Ping, TestErr>(|actor, _env, _err| {
            actor.model.errored = Some(true);
            Reply::ready()
        });

    // Result-based handler for Tally triggers TestErr2
    actor_builder
        .try_mutate_on::<Tally, (), TestErr2>(|_actor, _msg_ctx| {
            println!("Ping handler for Tally fired!");
            Box::pin(async { Err(TestErr2) })
        })
        .on_error::<Tally, TestErr2>(|actor, _env, _err| {
            assert!(
                actor.model.errored2.is_none(),
                "TestErr2 error handler called more than once!"
            );
            actor.model.errored2 = Some(true);
            Reply::ready()
        })
        .after_stop(|actor| {
            assert!(
                actor.model.errored.is_some(),
                "Error handler for TestErr was not called as expected (model.errored was not set)"
            );
            assert!(
                actor.model.errored2.is_some(),
                "Error handler for TestErr2 was not called as expected (model.errored2 was not set)"
            );
            Reply::ready()
        });

    let actor_handle = actor_builder.start().await;
    actor_handle.send(Ping).await;
    actor_handle.send(Tally {}).await;
    actor_handle.stop().await?;

    Ok(())
}

#[acton_test]
async fn test_fallible_handler_returns_value() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch();

    let actor_config =
        ActorConfig::new(Ern::with_root("fallible_return_demo").unwrap(), None, None)?;

    let mut actor_builder = runtime.new_actor_with_config::<Counter>(actor_config);

    // A fallible handler that returns a value on success
    actor_builder
        .try_mutate_on::<Increment, usize, TestErr>(|actor, _msg_ctx| {
            actor.model.count += 1;
            let current_count = actor.model.count;
            Box::pin(async move { Ok(current_count) })
        })
        .on_error::<Increment, TestErr>(|_, _, _| {
            // This should not be called
            panic!("on_error should not be called in this test");
        })
        .after_stop(|actor| {
            assert_eq!(
                actor.model.count, 1,
                "The counter should have been incremented."
            );
            Reply::ready()
        });

    let actor_handle = actor_builder.start().await;
    actor_handle.send(Increment).await;
    actor_handle.stop().await?;

    Ok(())
}
