# Broker and pub/sub

Use the broker when **many** listeners care about a fact and the publisher
should not know who they are. Use a direct `send` when one specific actor
needs the message. Broadcasting to a single known recipient adds a hop and
loses the reply path for nothing.

## Subscribing

Subscriptions must be registered on the builder, **before** `start()`.
A `subscribe` after the actor starts is silently ignored — no error, no
warning, just an actor that never hears anything.

```rust
let mut watcher = app.new_actor::<Watcher>();
watcher.mutate_on::<PriceChanged>(|actor, ctx| {
    actor.model.last = ctx.message().price;
    Reply::ready()
});
watcher.handle().subscribe::<PriceChanged>().await;   // before start
let handle = watcher.start().await;
```

## Broadcasting

```rust
let broker = actor.broker().clone();          // clone before the async block
broker.broadcast(PriceChanged { price }).await;
```

## Ordering: what is and is not guaranteed

**Per subscriber, broadcasts are ordered.** The broker's inbox is FIFO and its
`BrokerRequest` handler is a `mutate_on` that awaits fan-out before dequeuing
the next broadcast. So a given subscriber receives broadcasts in the order they
were published.

**Across subscribers, nothing is ordered.** Subscriber A may have processed
broadcast 2 while subscriber B is still on broadcast 1. Do not write logic that
assumes two subscribers are at the same point.

## Knowing a broadcast arrived

Broadcasts are fire-and-forget by design: `BrokerRequestEnvelope` carries only
the message `Arc`, with the reply address stripped, so a subscriber cannot
answer one. The broker can speak for the fan-out:

```rust
broker.broadcast(Tick).await;
broker.ask(FlushBroadcasts).await?;      // -> BroadcastsFlushed
```

`FlushBroadcasts` proves the broker has finished delivering to every
subscriber's inbox. It does **not** prove any subscriber has *processed* the
message. For that, ask the subscriber something afterwards; its FIFO inbox
makes the answer proof that the broadcast was handled first.

This two-step is how pub/sub tests stay deterministic without sleeping.

## Design note

A broadcast is a statement of fact about the past — `OrderPlaced`,
`PriceChanged`, `ConfigReloaded` — not an instruction. If you find yourself
broadcasting `DoTheThing` and expecting exactly one actor to act on it, you
wanted a direct send. Naming broadcasts as events keeps the coupling honest:
the publisher genuinely does not care who is listening, including nobody.
