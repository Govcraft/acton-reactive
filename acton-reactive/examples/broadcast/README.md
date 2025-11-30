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
.before_stop( | actor| {
let broker = actor.broker().clone();
let sum = actor.model.sum;
Reply::pending(async move {
broker.broadcast(PrintMessage(format ! ("Final sum: {sum}"))).await;
})
});
```

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
