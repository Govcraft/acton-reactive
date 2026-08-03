---
title: Testing Actors
description: Strategies for testing actor-based code without sleeping.
---

Actors are inherently testable. Their message-based interface makes it clear what inputs you can send and what behaviours to verify.

The one thing that makes actor tests hard is knowing *when* to assert. This page is mostly about that.

{% callout type="warning" title="Do not sleep to wait for a message" %}
`tokio::time::sleep` in a test is a guess about scheduling, and a guess that fails under load, on a busy CI machine, or on a different number of cores. It is the single largest source of flaky actor tests.

Acton's own documentation-example suite used to synchronise this way. Twenty-six of those tests turned out to be racy: measured over ten runs, individual tests failed anywhere from one to ten times out of ten. They now use the barriers on this page, and fail zero times in twenty.

There is always something real to wait for. The rest of this page is what.
{% /callout %}

## The acton_test helper

The workspace ships a companion crate, `acton_test`, which acton-reactive's own test suite uses. Its `#[acton_test]` attribute runs your async test on a Tokio runtime and installs a panic hook that captures panics from spawned actor tasks, so a panicking handler fails the test with the original message and location instead of going unnoticed.

```toml
[dev-dependencies]
acton_test = "9"
```

```rust
use acton_test::prelude::*;

#[acton_test]
async fn test_counter_increments() {
    // test body as usual
}
```

Plain `#[tokio::test]` also works, but `#[acton_test]` gives better failure reporting when actors panic.

---

## The main barrier: ask

`ask` sends a request and waits for the actor's reply. Because inboxes are FIFO, **a completed `ask` proves every message sent to that actor beforehand has been processed.**

That single property replaces most sleeps in test code:

```rust
#[acton_test]
async fn test_counter_increments() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let mut counter = runtime.new_actor::<Counter>();

    counter
        .mutate_on::<Increment>(|actor, _env| {
            actor.model.count += 1;
            Reply::ready()
        })
        .act_on::<GetCount>(|actor, env| {
            let count = actor.model.count;
            let reply = env.reply_envelope();
            Reply::pending(async move {
                reply.send(Count(count)).await;
            })
        });

    let handle = counter.start().await;

    handle.send(Increment).await;
    handle.send(Increment).await;

    // No sleep. The reply cannot arrive until both increments have been handled.
    let count = handle.ask(GetCount).await?;
    assert_eq!(count.0, 2);

    runtime.shutdown_all().await?;
    Ok(())
}
```

The message needs a `Request` impl to be askable:

```rust
#[acton_message]
struct GetCount;

#[acton_message]
struct Count(i32);

impl Request for GetCount {
    type Response = Count;
}
```

{% callout type="note" title="Probe actors are rarely needed now" %}
Earlier versions required a second "probe" actor with an atomic counter and a hand-addressed envelope just to observe a reply. `ask` gives you the reply directly. Reach for a probe only when you are testing something `ask` genuinely cannot express: several replies to one request, or an actor-to-actor exchange you want to observe from the side.
{% /callout %}

---

## Testing work that fans out

When the actor under test sends to *another* actor, asking the first one proves only that it dispatched. Ask the actor that does the work:

```rust
#[acton_test]
async fn test_producer_consumer() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;

    let mut consumer = runtime.new_actor::<Consumer>();
    consumer
        .mutate_on::<Item>(|actor, _env| {
            actor.model.received += 1;
            Reply::ready()
        })
        .act_on::<GetReceived>(|actor, env| {
            let received = actor.model.received;
            let reply = env.reply_envelope();
            Reply::pending(async move { reply.send(Received(received)).await })
        });
    let consumer_handle = consumer.start().await;

    let mut producer = runtime.new_actor::<Producer>();
    producer.model.consumer = Some(consumer_handle.clone());
    producer.mutate_on::<Produce>(|actor, env| {
        let count = env.message().count;
        let consumer = actor.model.consumer.clone().unwrap();
        Reply::pending(async move {
            for _ in 0..count {
                consumer.send(Item).await;
            }
        })
    });
    let producer_handle = producer.start().await;

    // 1. Wait for the producer to finish sending. This is a `mutate_on`
    //    handler, so the actor awaits it before taking the next message,
    //    and this ask therefore lands strictly after the sends.
    producer_handle.ask(HasProduced).await?;

    // 2. Then wait for the consumer to work through what it was sent.
    let received = consumer_handle.ask(GetReceived).await?;
    assert_eq!(received.0, 5);

    runtime.shutdown_all().await?;
    Ok(())
}
```

Two asks, because there are two actors. Each one only speaks for its own inbox.

{% callout type="warning" title="act_on handlers carry a weaker guarantee" %}
A `mutate_on` handler's `Reply::pending` future is awaited **inline**, before the actor takes its next message. An `act_on` handler's future is drained **concurrently**, so a later `ask` to that actor can be answered while the earlier handler's future is still running.

If you need "this handler's async work is finished", make it a `mutate_on`, or have the work itself report completion.
{% /callout %}

---

## Testing pub/sub

`broadcast` completes when the **broker** has the message, not when subscribers do. Asking a subscriber does not help either: the broker strips the reply address, so a subscriber cannot answer a broadcast.

The broker is the only participant that can speak for one. Ask it to flush:

```rust
#[acton_test]
async fn test_broadcast() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let mut handles = Vec::new();
    for _ in 0..3 {
        let mut subscriber = runtime.new_actor::<Subscriber>();
        subscriber
            .mutate_on::<Event>(|actor, _env| {
                actor.model.seen += 1;
                Reply::ready()
            })
            .act_on::<GetSeen>(|actor, env| {
                let seen = actor.model.seen;
                let reply = env.reply_envelope();
                Reply::pending(async move { reply.send(Seen(seen)).await })
            });

        subscriber.handle().subscribe::<Event>().await;
        handles.push(subscriber.start().await);
    }

    broker.broadcast(Event).await;

    // The reply cannot arrive until every earlier broadcast is sitting in
    // every subscriber's inbox.
    broker.ask(FlushBroadcasts).await?;

    // Delivery, not completion. Ask each subscriber to establish that it has
    // worked through the event now queued ahead of the question.
    for handle in &handles {
        assert_eq!(handle.ask(GetSeen).await?.0, 1);
    }

    runtime.shutdown_all().await?;
    Ok(())
}
```

Two steps, because they prove different things:

1. **`FlushBroadcasts`** proves the event has been *delivered* to every subscriber's inbox.
2. **`ask` on a subscriber** proves that subscriber has *handled* it, because the event is now ahead of your question in its inbox.

{% callout type="note" title="shutdown_all flushes for you" %}
`shutdown_all` asks the broker to flush before it signals anything, so a test that broadcasts and then shuts down does not need its own `FlushBroadcasts`. You need one when you assert *before* shutting down, as above, or when the broadcast has not been issued yet at the moment shutdown begins.
{% /callout %}

---

## Waiting for a supervised child

Do not sleep waiting for a restart. `SupervisedChild` publishes what you need:

```rust
let worker = supervisor
    .supervise_with::<Worker>(&runtime, config, blueprint)
    .await?;

// Wait for the first incarnation to be up.
let first = worker.wait_running().await?;

first.stop().await?;

// Wait for the specific incarnation the restart produces.
let second = worker.wait_generation(RestartGeneration::FIRST.next()).await?;
assert_eq!(second.id(), first.id(), "a restart keeps the child's identity");
```

Counting blueprint invocations is a good way to prove a *new* incarnation exists rather than the old one having been revived, since a restart deliberately keeps the child's identifier:

```rust
let builds = Arc::new(AtomicUsize::new(0));
let blueprint = {
    let builds = Arc::clone(&builds);
    move |actor: &mut ManagedActor<Idle, Worker>| {
        builds.fetch_add(1, Ordering::SeqCst);
        actor.mutate_on::<Task>(handle_task);
    }
};
// ... after the restart:
assert_eq!(builds.load(Ordering::SeqCst), 2);
```

---

## When there is genuinely nothing to ask

Some work has no natural reply point: a fire-and-forget pipeline, an actor that only emits. Two options, in order of preference:

**Give the actor something to answer.** A `GetStatus` handler that reports a counter costs a few lines and turns a timing guess into a fact. This is the recommended approach, and it is what Acton's own test suite does.

**Have the actor hold the reply envelope.** When the answer genuinely is not ready yet, store `ctx.reply_envelope()` in the model and send the reply when the work completes. The caller's `ask` simply waits, and the result stops depending on whether the question arrived first:

```rust
.mutate_on::<AwaitDone>(|actor, ctx| {
    let reply = ctx.reply_envelope();
    if actor.model.done {
        Reply::pending(async move { reply.send(Finished).await })
    } else {
        actor.model.waiting = Some(reply);
        Reply::ready()
    }
})
.mutate_on::<WorkFinished>(|actor, _ctx| {
    actor.model.done = true;
    match actor.model.waiting.take() {
        Some(reply) => Reply::pending(async move { reply.send(Finished).await }),
        None => Reply::ready(),
    }
});
```

{% callout type="note" title="A sleep inside a handler is a different thing" %}
A sleep in *caller* position is a synchronisation guess. A sleep *inside a handler* may be the subject being modelled: a slow service, an async connect, work you want to demonstrate is concurrent. Those are fine. It is waiting on the clock instead of on the system that causes flakes.
{% /callout %}

---

## Bound your waits

Even a correct barrier can hang if the code under test is broken. A test that hangs tells you far less than a test that fails, and it blocks CI. Wrap waits in a timeout generous enough never to fire on a healthy run:

```rust
const PATIENCE: Duration = Duration::from_secs(5);

let count = tokio::time::timeout(PATIENCE, handle.ask(GetCount))
    .await
    .expect("the counter must answer")?;
```

`ask` already carries a 30-second default deadline (`DEFAULT_ASK_TIMEOUT`), and `ask_with_timeout` sets your own. The distinction matters when you are testing the failure itself: `AskError::TimedOut` means the actor still holds a live reply address and has not answered; `AskError::NoReply` means no answer is coming at all.

---

## Test helpers

```rust
async fn setup_counter(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut counter = runtime.new_actor::<Counter>();
    counter
        .mutate_on::<Increment>(handle_increment)
        .act_on::<GetCount>(handle_get);
    counter.start().await
}

#[acton_test]
async fn test_with_helper() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let counter = setup_counter(&mut runtime).await;

    counter.send(Increment).await;
    assert_eq!(counter.ask(GetCount).await?.0, 1);

    runtime.shutdown_all().await?;
    Ok(())
}
```

---

## Isolate each test

Each test gets its own runtime, so nothing leaks between them:

```rust
#[acton_test]
async fn test_one() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;  // isolated
    // ...
    runtime.shutdown_all().await?;
    Ok(())
}
```

---

## Is my test actually testing anything?

A test that passes is not proof it would catch the failure it names. The check is cheap: **break the thing on purpose and confirm the test fails.**

Comment out the increment, and a counter test that still passes was never reading the counter. Acton's own suite found several tests passing for reasons other than the one in their name this way.

Run the mutation, not just the suite.

---

## Summary

- Use `#[acton_test]` for async tests; it reports handler panics properly
- **`ask` is the barrier.** A reply proves the handler ran, and everything queued before it was processed
- **One ask speaks for one actor.** Fan-out needs one per actor
- **`mutate_on` awaits its future inline; `act_on` does not**
- **Broadcasts need `broker.ask(FlushBroadcasts)`** for delivery, then an `ask` on a subscriber for completion
- **`SupervisedChild::wait_running` / `wait_generation`** for restarts
- Bound every wait with a timeout so a failure fails instead of hanging
- Each test gets its own runtime, cleaned up with `shutdown_all()`
- Never sleep to wait for a message

---

## Continue Learning

You've covered the Building Apps section:
- Parent-child actors
- Request-response patterns
- Error handling
- Testing strategies

Continue to [Advanced](/docs/advanced/ipc) for topics like IPC and performance.
