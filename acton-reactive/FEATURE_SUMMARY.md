# Acton Reactive: Concurrent Rust Made Simple

Concurrent Rust is powerful but demanding—managing `Arc<Mutex<T>>`, avoiding data races, and coordinating async tasks requires significant expertise. Acton Reactive offers a simpler mental model: actors. Each actor owns its state exclusively, communicates through message passing, and runs independently. The framework handles coordination, letting you focus on business logic while Rust's type system enforces safety at compile time.

## Key Features

### Derive Macros Eliminate Boilerplate
Define actors and messages with simple attributes:

```rust
#[acton_actor]
struct Counter { count: i32 }

#[acton_message]
struct Increment;
```

The macros generate all required trait implementations, letting you focus on domain logic rather than framework plumbing.

### Two Handler Types for State Management
- **`mutate_on<M>`** — Handlers that modify state run sequentially, ensuring consistency
- **`act_on<M>`** — Read-only handlers can run concurrently for better throughput

This distinction is enforced at the API level, making state management intentions explicit.

### Async-Native on Tokio
Built on Tokio from the ground up—not retrofitted. All message handling is async, actor lifecycles are non-blocking, and the runtime efficiently schedules work across available cores.

### Pub/Sub Messaging via Broker
Actors can subscribe to message types and receive broadcasts, enabling loose coupling:

```rust
handle.subscribe::<StatusUpdate>().await;
broker.broadcast(StatusUpdate::Ready).await;
```

### Lifecycle Hooks for Resource Management
`before_start`, `after_start`, `before_stop`, `after_stop`—initialize resources, announce readiness, persist state, and clean up gracefully.

### Supervision for Fault Tolerance
Parent actors supervise children with configurable restart strategies. Fault isolation prevents cascading failures across your system.

### Optional IPC for Cross-Process Communication
Send messages to actors from external processes via Unix Domain Sockets (enabled with `ipc` feature):
- Type-safe serialization with `#[acton_message(ipc)]`
- Actor exposure by logical name for routing
- Wire protocol with correlation IDs for request/response patterns

## Best For / Consider Alternatives

**Good fit:**
- Stateful services with complex internal logic
- Event-driven systems with clear component boundaries
- Applications modeling entities (users, sessions, devices)
- Systems needing fault isolation between components

**Consider alternatives:**
- Pure computation without state → async functions
- Simple shared counters → `Arc<AtomicUsize>`
- Request-response without state → direct function calls
- WASM targets → not currently supported

## Maturity Indicators

- Comprehensive documentation with API docs, examples, and guides
- Consistent API design with builder patterns throughout
- Proper error handling with `Result` types—no panics in normal operation
- Dedicated testing infrastructure (`acton-test` crate)
- Dual MIT/Apache-2.0 licensing

## Next Steps

```toml
[dependencies]
acton-reactive = "7"
```

Tokio is re-exported via the prelude—no separate dependency needed. See the [examples directory](https://github.com/Govcraft/acton-reactive/tree/main/acton-reactive/examples) for working code demonstrating common patterns.
