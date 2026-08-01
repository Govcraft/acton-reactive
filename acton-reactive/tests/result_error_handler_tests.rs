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
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let actor_config = ActorConfig::new(Ern::with_root("error_handler_demo").unwrap(), None);

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

/// Error type carrying a value so tests can verify the error handler receives
/// the exact error the failing handler returned.
#[derive(Debug, Clone)]
struct ReadOnlyErr(usize);

impl std::fmt::Display for ReadOnlyErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Deliberate read-only test error ({})", self.0)
    }
}
impl std::error::Error for ReadOnlyErr {}

/// State for verifying concurrent execution of fallible read-only handlers.
#[derive(Debug, Default, Clone)]
struct ConcurrentFallible {
    pub error_count: usize,
    pub started_at: Option<std::time::Instant>,
}

/// A read-only query that sleeps before optionally failing.
#[derive(Clone, Debug)]
struct SlowQuery {
    should_fail: bool,
}

#[acton_test]
async fn test_try_act_on_error_handler_fires() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let actor_config = ActorConfig::new(
        Ern::with_root("read_only_error_handler_demo").unwrap(),
        None,
    );

    let mut actor_builder = runtime.new_actor_with_config::<Counter>(actor_config);

    // Result-based read-only handler for Ping that always fails
    actor_builder
        .try_act_on::<Ping, (), ReadOnlyErr>(|_actor, _msg_ctx| {
            Box::pin(async { Err(ReadOnlyErr(42)) })
        })
        .on_error::<Ping, ReadOnlyErr>(|actor, _env, err| {
            assert_eq!(err.0, 42, "Error handler received the wrong error value");
            actor.model.errored = Some(true);
            Reply::ready()
        })
        .after_stop(|actor| {
            assert!(
                actor.model.errored.is_some(),
                "Error handler for try_act_on was not called as expected (model.errored was not set)"
            );
            Reply::ready()
        });

    let actor_handle = actor_builder.start().await;
    actor_handle.send(Ping).await;
    actor_handle.stop().await?;

    Ok(())
}

#[acton_test]
async fn test_try_act_on_unhandled_error_logs_and_continues() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let actor_config = ActorConfig::new(
        Ern::with_root("read_only_unhandled_error_demo").unwrap(),
        None,
    );

    let mut actor_builder = runtime.new_actor_with_config::<Counter>(actor_config);

    // A failing read-only handler with NO on_error registration: the error must be
    // logged and dropped without panicking or hanging the actor.
    actor_builder
        .try_act_on::<Ping, (), ReadOnlyErr>(|_actor, _msg_ctx| {
            Box::pin(async { Err(ReadOnlyErr(7)) })
        })
        .mutate_on::<Increment>(|actor, _msg_ctx| {
            actor.model.count += 1;
            Reply::ready()
        })
        .after_stop(|actor| {
            assert_eq!(
                actor.model.count, 1,
                "Actor should keep processing messages after an unhandled read-only error"
            );
            Reply::ready()
        });

    let actor_handle = actor_builder.start().await;
    actor_handle.send(Ping).await;
    actor_handle.send(Increment).await;
    actor_handle.stop().await?;

    Ok(())
}

#[acton_test]
async fn test_try_act_on_handlers_remain_concurrent() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let actor_config =
        ActorConfig::new(Ern::with_root("read_only_concurrency_demo").unwrap(), None);

    let mut actor_builder = runtime.new_actor_with_config::<ConcurrentFallible>(actor_config);
    actor_builder.model.started_at = Some(std::time::Instant::now());

    actor_builder
        .try_act_on::<SlowQuery, (), TestErr>(|_actor, msg_ctx| {
            let should_fail = msg_ctx.message().should_fail;
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                if should_fail {
                    Err(TestErr)
                } else {
                    Ok(())
                }
            })
        })
        .on_error::<SlowQuery, TestErr>(|actor, _env, _err| {
            // Mutable access here proves the error was dispatched at a flush point.
            actor.model.error_count += 1;
            Reply::ready()
        })
        .after_stop(|actor| {
            let elapsed = actor.model.started_at.unwrap().elapsed();
            assert_eq!(
                actor.model.error_count, 5,
                "Every failing concurrent handler should reach the error handler"
            );
            // Ten 300ms handlers executed sequentially would take 3s; concurrent
            // execution should finish well under half of that.
            assert!(
                elapsed < std::time::Duration::from_millis(1500),
                "Read-only fallible handlers should still run concurrently. Took: {elapsed:?}"
            );
            Reply::ready()
        });

    let actor_handle = actor_builder.start().await;
    for i in 0..10 {
        actor_handle
            .send(SlowQuery {
                should_fail: i % 2 == 0,
            })
            .await;
    }
    actor_handle.stop().await?;

    Ok(())
}

#[acton_test]
async fn test_fallible_handler_returns_value() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let actor_config = ActorConfig::new(Ern::with_root("fallible_return_demo").unwrap(), None);

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
