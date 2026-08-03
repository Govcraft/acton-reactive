# Testing actors deterministically

The rule: **never synchronise with `sleep`.** A sleep is slow when it works and
flaky when it does not, and it hides the bug it was added to paper over. Every
sleep has a barrier that replaces it.

```toml
[dev-dependencies]
acton_test = "9"
```

## The main barrier: `ask`

`ask` resolves on the first reply. Mailboxes are FIFO, so a completed `ask`
proves that **everything sent to that actor beforehand has been processed**.
That single property replaces most sleeps.

```rust
#[acton_test]
async fn increments_are_applied() -> anyhow::Result<()> {
    let mut app = ActonApp::launch_async().await;
    let handle = build_counter(&mut app).await;

    handle.send(Increment).await;
    handle.send(Increment).await;

    // Not a sleep: this cannot resolve until both Increments are processed.
    let count = handle.ask(GetCount).await?;
    assert_eq!(count.value, 2);

    app.shutdown_all().await
}
```

If your actor has no natural query message, add one. A test-visible `GetState`
request is cheaper than a flaky suite.

## Where the barrier does not reach

**Fan-out needs one ask per actor.** An `ask` to actor A says nothing about
actor B's inbox. Asking each one is the barrier; asking one and hoping is not.

**`act_on` only proves the work started.** A `mutate_on` handler's
`Reply::pending` future is awaited inline before the actor takes its next
message, so a reply from it means the async work finished. An `act_on`
handler's future is drained concurrently, so a reply from it means only that
the handler ran. If a test needs the *effect*, make the handler `mutate_on`, or
ask for the effect rather than for the acknowledgement.

**Broadcasts have no reply path.** `BrokerRequestEnvelope` carries only the
message; the reply address is stripped, so a subscriber cannot answer a
broadcast. Only the broker can speak for one:

```rust
broker.broadcast(PriceChanged { .. }).await;
broker.ask(FlushBroadcasts).await?;        // fan-out has completed
subscriber.ask(GetLastPrice).await?;       // the subscriber has processed it
```

Both steps are needed. The flush proves delivery; the second ask proves the
subscriber drained its inbox.

**Restarts:** `wait_running()` for "it is up again", `wait_generation(n)` for
"it is up again *as the n-th incarnation*". Use the second when the test cares
that a restart actually happened rather than that the actor merely exists.

## The deferred-reply pattern

When an actor genuinely cannot answer yet, stash the envelope instead of
sleeping the caller:

```rust
#[acton_actor]
struct Connector {
    waiting: Option<OutboundEnvelope>,
    ready: bool,
}

connector.mutate_on::<AreYouReady>(|actor, ctx| {
    if actor.model.ready {
        let reply = ctx.reply_envelope();
        Reply::pending(async move { reply.send(Ready).await })
    } else {
        actor.model.waiting = Some(ctx.reply_envelope());
        Reply::ready()          // the caller waits on ask, not on a timer
    }
});
```

This is also the correct fix for "`after_start` has not finished yet", because
`after_start` returning `Reply::pending` does not hold the actor back.

## Bounded waits, when you truly need one

For a condition with no message behind it (a file appearing, an external
process), poll with a bound and fail loudly rather than sleeping a guess:

```rust
const PATIENCE: Duration = Duration::from_secs(5);
```

Reach for this only when there is genuinely no barrier available.

## Is the test testing anything?

Actor tests fail silently in a specific way: they pass because nothing ran, not
because the behaviour is right. Before trusting a green test, **mutate the code
it names** — break the handler it claims to cover and confirm the test goes
red. A test that stays green is measuring the framework's ability to start up,
not your logic.

Watch for assertions on a state that was never reached, `assert!(result.is_ok())`
where the interesting content is inside `result`, and tests whose only barrier
is `shutdown_all`.
