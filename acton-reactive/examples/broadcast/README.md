# Broadcasting Example

This example demonstrates how multiple actors can communicate using a central broadcaster in the Acton framework.
Instead of sending messages directly to each other, actors can broadcast messages, allowing any subscribed actors to
react accordingly. This helps decouple the actors from one another, enabling a more flexible and maintainable messaging
flow.

In this example, we have three actors:

- **DataCollector**: Collects numerical data.
- **Aggregator**: Maintains a running total of the data collected.
- **Printer**: Responsible for all screen output.

## Key Features Demonstrated

- **Broadcasting Messages**: Actors use a central broadcaster to send and receive messages, enabling multiple actors to
  react to the same events without direct connections.
- **Actor Subscriptions**: Actors subscribe to messages they're interested in, allowing them to receive only relevant
  broadcasts.
- **Decoupled Communication**: The example showcases how actors remain loosely coupled by using the broadcaster instead
  of direct communication.

## How It Works

- The `DataCollector` actor collects incoming numerical data and broadcasts each data point.
- The `Aggregator` actor listens for data points and maintains a running total, broadcasting updates whenever new data
  arrives.
- The `Printer` actor listens for any messages that need to be displayed on the screen and outputs them accordingly.

## Running the Example

To run the example:

1. Start the Acton application and launch all actors.
2. Send data messages to the `DataCollector`.
3. Watch as the `Aggregator` updates the sum and sends status updates, all displayed by the `Printer`.

## Code Walkthrough

### Initializing the App and Actors

```rust
// Launch the app
let mut app = ActonApp::launch();

// Initialize each actor
let mut data_collector = app.initialize::<DataCollector>().await;
let mut aggregator = app.initialize::<Aggregator>().await;
let mut printer = app.initialize::<Printer>().await;
```

We launch the application and initialize our actors. Each actor is then configured with its behavior.

### DataCollector Actor

The `DataCollector` receives new data and broadcasts it:

```rust
data_collector
.act_on::<NewData>( | actor, envelope| {
actor.model.data_points.push(envelope.message().0);

let broker = actor.broker().clone();
let message = format ! ("DataCollector received new data: {}", envelope.message().0.clone());
Reply::pending(async move { broker.broadcast(PrintMessage(message)).await })
})
.after_start( | actor| {
let broker = actor.broker().clone();
Reply::pending(async move {
broker.broadcast(PrintMessage("DataCollector is ready to collect data!".to_string())).await;
})
});
```

### Aggregator Actor

The `Aggregator` maintains a running total of all data received:

```rust
aggregator
.act_on::<NewData>( | actor, envelope| {
actor.model.sum += envelope.message().0;

let broker = actor.broker().clone();
let sum = actor.model.sum;
let message = format! ("Aggregator updated sum: {}", sum);

Reply::pending(async move {
broker.broadcast(PrintMessage(message)).await;
})
})
.after_start( | actor| {
let broker = actor.broker().clone();
Reply::pending(async move {
broker.broadcast(PrintMessage("Aggregator is ready to sum data!".to_string())).await;
})
})
.mutate_on::<Finalize>( | actor, _envelope| {
let broker = actor.broker().clone();
let sum = actor.model.sum;
Reply::pending(async move {
broker.broadcast(PrintMessage(format ! ("Final sum: {sum}"))).await;
broker.broadcast(StatusUpdate::Finished).await;
})
});
```

Note that the final sum is reported in response to an explicit `Finalize` message,
**not** from a `before_stop` hook. A hook that broadcasts during shutdown races the
shutdown: the message still has to cross the broker to reach the `Printer`, and the
`Printer` may already have closed its inbox by then. Reporting before shutdown starts
removes the race instead of narrowing it.

### Knowing when the pipeline has finished

`broadcast` returns as soon as the message reaches the broker, which says nothing about
whether any subscriber has run — so shutting down straight afterwards races the work, and
a `sleep` only lengthens the fuse. The example instead closes the pipeline with a marker
message and then asks the actor at the end of it:

```rust
broker_handle.broadcast(NewData(5)).await;
broker_handle.broadcast(NewData(10)).await;
broker_handle.broadcast(Finalize).await;

// Returns only once the Printer has printed the final sum and both
// collectors have reported in.
let report: FinalReport = printer_handle.ask(AwaitReport).await?;

runtime.shutdown_all().await?;
```

This is exact rather than probabilistic, because the broker delivers broadcasts in order
and a `mutate_on` handler finishes its `Reply::pending` future before taking its next
message. The `Printer` holds on to the reply envelope when the request arrives early, so
the answer comes at the right moment no matter how the timing falls.

### Printer Actor

The `Printer` handles all output:

```rust
printer
.act_on::<PrintMessage>( | _actor, envelope| {
println ! ("Printer received: {}", envelope.message().0);
Reply::ready()
})
.after_start( | actor| {
let broker = actor.broker().clone();
Reply::pending(async move {
broker.broadcast(PrintMessage("Printer is ready to display messages!".to_string())).await;
})
});
```

### Subscribing to Messages

Actors subscribe to the messages they're interested in:

```rust
data_collector.handle().subscribe::<NewData>().await;
aggregator.handle().subscribe::<NewData>().await;
printer.handle().subscribe::<PrintMessage>().await;
```

### Sending Messages via the Broadcaster

Finally, messages are broadcasted:

```rust
broker.broadcast(NewData(5)).await;
broker.broadcast(NewData(10)).await;

// Demonstrate sending a direct message to the Printer
printer_handle.send_message(PrintMessage("Printing is fun!".to_string())).await;
```

## Running the Example

Run this example with:

```bash
cargo run --example broadcasting
```

You'll see the `Printer` displaying messages as they're broadcast by other actors.
