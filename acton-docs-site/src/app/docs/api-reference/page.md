---
title: API Reference
nextjs:
  metadata:
    title: API Reference - acton-reactive
    description: Complete API documentation for all public types, traits, and functions in the acton-reactive crate.
---

Complete API documentation for all public types, traits, and functions in the `acton-reactive` crate.

---

## Prelude

The prelude module provides convenient access to the most commonly used types:

```rust
use acton_reactive::prelude::*;
```

### Included Types

| Type | Module | Description |
|------|--------|-------------|
| `ActonApp` | `common` | Entry point for system initialization |
| `ActorRuntime` | `common` | Runtime manager for actor system |
| `ManagedActor` | `actor` | Core actor wrapper with type-state pattern |
| `Idle` | `actor` | Type-state marker for configuration phase |
| `Started` | `actor` | Type-state marker for active actor |
| `ActorHandle` | `common` | External reference to actor |
| `ActorBroker` | `common` | Central pub/sub message broker |
| `ActorConfig` | `actor` | Configuration builder for actors |
| `MessageAddress` | `message` | Actor endpoint address |
| `OutboundEnvelope` | `message` | Message wrapper for sending |
| `BrokerRequest` | `message` | Broadcast message wrapper |
| `ActorReply` | `common` | Utility for handler return types |

### Included Traits

| Trait | Description |
|-------|-------------|
| `ActonMessage` | Marker trait for message payloads |
| `ActorHandleInterface` | Primary trait for actor interaction |
| `Broker` | Message broadcasting capability |
| `Subscriber` | Broker access capability |
| `Subscribable` | Subscription management |

---

## Core Types

### ActonApp

Entry point for initializing the actor system.

```rust
pub struct ActonApp;

impl ActonApp {
    /// Initialize the actor runtime
    pub fn launch() -> ActorRuntime;
}
```

**Example:**
```rust
let runtime = ActonApp::launch();
```

---

### ActorRuntime

Manages the active system and provides methods for creating top-level actors.

```rust
pub struct ActorRuntime {
    pub broker: ActorHandle,
    // ... internal fields
}

impl ActorRuntime {
    /// Create a new actor with default name
    pub fn new_actor<Model>(&mut self) -> ManagedActor<Idle, Model>
    where
        Model: Default + Clone + Send + Sync + 'static;

    /// Create a new actor with custom name
    pub fn new_actor_with_name<Model>(
        &mut self,
        name: String,
    ) -> ManagedActor<Idle, Model>
    where
        Model: Default + Clone + Send + Sync + 'static;

    /// Gracefully shutdown all actors
    pub async fn shutdown_all(&mut self) -> anyhow::Result<()>;

    /// Get the system broker
    pub fn get_broker(&self) -> &ActorHandle;
}
```

**Example:**
```rust
let mut runtime = ActonApp::launch();
let mut actor = runtime.new_actor::<MyState>();
// ... configure and start actor
runtime.shutdown_all().await?;
```

---

### ManagedActor<ActorState, Model>

Core actor wrapper using the type-state pattern to enforce valid operations at compile time.

#### Type Parameters

- `ActorState`: Either `Idle` or `Started`
- `Model`: User-defined state type (must implement `Default + Clone + Send + Sync + 'static`)

#### Methods (Idle State)

```rust
impl<Model> ManagedActor<Idle, Model> {
    /// Register a mutable message handler
    pub fn mutate_on<M>(
        &mut self,
        handler: impl Fn(&mut ManagedActor<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register a read-only message handler (concurrent execution)
    pub fn act_on<M>(
        &mut self,
        handler: impl Fn(&ManagedActor<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register a fallible mutable handler
    pub fn mutate_on_fallible<M>(
        &mut self,
        handler: impl Fn(&mut ManagedActor<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = Result<Box<dyn ActonMessageReply>, Box<dyn Error>>>
            + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register a fallible read-only handler
    pub fn act_on_fallible<M>(
        &mut self,
        handler: impl Fn(&ManagedActor<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = Result<Box<dyn ActonMessageReply>, Box<dyn Error>>>
            + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register an error handler for a specific message and error type
    pub fn on_error<M, E>(
        &mut self,
        handler: impl Fn(&mut ManagedActor<Started, Model>, &mut MessageContext<M>, &E)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage,
        E: Error + 'static;

    /// Lifecycle hook: runs before message loop starts
    pub fn before_start(
        &mut self,
        hook: impl Fn(&ManagedActor<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Lifecycle hook: runs after message loop starts
    pub fn after_start(
        &mut self,
        hook: impl Fn(&ManagedActor<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Lifecycle hook: runs before message loop stops
    pub fn before_stop(
        &mut self,
        hook: impl Fn(&ManagedActor<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Lifecycle hook: runs after message loop stops
    pub fn after_stop(
        &mut self,
        hook: impl Fn(&ManagedActor<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Start the actor, transitioning from Idle to Started
    pub async fn start(self) -> ActorHandle;
}
```

#### Methods (Started State)

```rust
impl<Model> ManagedActor<Started, Model> {
    /// Get the actor's state
    pub fn model(&self) -> &Model;

    /// Get mutable access to the actor's state
    pub fn model_mut(&mut self) -> &mut Model;

    /// Create a new outbound envelope for sending messages
    pub fn new_envelope(&self) -> OutboundEnvelope;

    /// Get the actor's handle
    pub fn handle(&self) -> &ActorHandle;

    /// Access supervised children
    pub fn children(&self) -> &DashMap<String, ActorHandle>;

    /// Supervise a new child actor
    pub async fn supervise<ChildModel>(
        &self,
        child: ManagedActor<Idle, ChildModel>,
    ) -> anyhow::Result<ActorHandle>
    where
        ChildModel: Default + Clone + Send + Sync + 'static;

    /// Find a child by Ern
    pub fn find_child(&self, id: &Ern) -> Option<ActorHandle>;
}
```

**Example:**
```rust
use acton_macro::{acton_actor, acton_message};

#[acton_actor]
struct CounterState {
    count: u32,
}

#[acton_message]
struct Increment(u32);

let mut actor = runtime.new_actor::<CounterState>();

actor
    .mutate_on::<Increment>(|actor, ctx| {
        Box::pin(async move {
            actor.model_mut().count += ctx.message().0;
        })
    })
    .before_start(|actor| {
        Box::pin(async move {
            println!("Actor starting with count: {}", actor.model().count);
        })
    });

let handle = actor.start().await;
```

---

### ActorHandle

External reference to an actor for sending messages and managing lifecycle.

```rust
pub struct ActorHandle {
    // ... internal fields
}

impl ActorHandle {
    /// Send a message to this actor
    pub async fn send(&self, message: impl ActonMessage);

    /// Send a message synchronously (spawns runtime if needed)
    pub fn send_sync(&self, message: impl ActonMessage, recipient: &MessageAddress);

    /// Stop the actor gracefully
    pub async fn stop(&self) -> anyhow::Result<()>;

    /// Get the actor's Ern identifier
    pub fn id(&self) -> Ern;

    /// Get the actor's name
    pub fn name(&self) -> String;

    /// Get the actor's reply address
    pub fn reply_address(&self) -> MessageAddress;

    /// Create an envelope for sending to a specific recipient
    pub fn create_envelope(&self, recipient: &MessageAddress) -> OutboundEnvelope;

    /// Access supervised children
    pub fn children(&self) -> &DashMap<String, ActorHandle>;

    /// Find a child by Ern
    pub fn find_child(&self, id: &Ern) -> Option<ActorHandle>;

    /// Clone the handle reference
    pub fn clone_ref(&self) -> Self;
}
```

---

### ActorBroker

Central pub/sub message broker for broadcasting messages to subscribers.

```rust
pub struct ActorBroker {
    // Derefs to ActorHandle
}

impl Deref for ActorBroker {
    type Target = ActorHandle;
}

impl ActorBroker {
    /// Broadcast a message to all subscribers of that message type
    pub async fn broadcast(&self, message: impl ActonMessage);

    /// Broadcast synchronously
    pub fn broadcast_sync(&self, message: impl ActonMessage) -> anyhow::Result<()>;
}
```

---

## Traits

### ActonMessage

Marker trait for types that can be sent as messages between actors.

```rust
pub trait ActonMessage: DynClone + Any + Send + Sync + Debug {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
```

**Automatic Implementation:**

Any type implementing `Clone + Debug + Send + Sync + 'static` automatically implements `ActonMessage`.

**Example (using macro - recommended):**
```rust
use acton_macro::acton_message;

#[acton_message]
struct MyMessage {
    data: String,
}
// acton_message derives Clone + Debug and asserts Send + Sync + 'static
```

---

### ActorHandleInterface

Primary trait for actor interaction, implemented by `ActorHandle` and `ActorBroker`.

```rust
pub trait ActorHandleInterface: Clone + Debug + Send + Sync + 'static {
    /// Get the actor's reply address
    fn reply_address(&self) -> MessageAddress;

    /// Send a message to the actor
    fn send(&self, message: impl ActonMessage) -> impl Future<Output = ()> + Send + Sync;

    /// Send synchronously
    fn send_sync(&self, message: impl ActonMessage, recipient: &MessageAddress);

    /// Stop the actor
    fn stop(&self) -> impl Future<Output = anyhow::Result<()>> + Send + Sync;

    /// Access children
    fn children(&self) -> &DashMap<String, ActorHandle>;

    /// Find a child
    fn find_child(&self, id: &Ern) -> Option<ActorHandle>;

    /// Get actor identifier
    fn id(&self) -> Ern;

    /// Get actor name
    fn name(&self) -> String;

    /// Clone the handle
    fn clone_ref(&self) -> Self;

    // IPC methods (feature-gated)
    #[cfg(feature = "ipc")]
    fn send_boxed(&self, message: Box<dyn ActonMessage>) -> impl Future<Output = ()>;

    #[cfg(feature = "ipc")]
    fn try_send_boxed(&self, message: Box<dyn ActonMessage>)
        -> Result<(), TrySendError<Envelope>>;
}
```

---

### Broker

Trait for message broadcasting capability.

```rust
pub trait Broker: Clone + Debug + Default + Send + Sync + 'static {
    /// Broadcast a message to all subscribers
    fn broadcast(&self, message: impl ActonMessage)
        -> impl Future<Output = ()> + Send + Sync;

    /// Broadcast synchronously
    fn broadcast_sync(&self, message: impl ActonMessage) -> anyhow::Result<()>
    where
        Self: ActorHandleInterface + Sized;
}
```

---

### Subscribable

Trait for managing pub/sub subscriptions.

```rust
pub trait Subscribable: ActorHandleInterface + Subscriber {
    /// Subscribe to a message type
    fn subscribe<M>(&self) -> impl Future<Output = ()> + Send + Sync
    where
        M: ActonMessage;

    /// Unsubscribe from a message type
    fn unsubscribe<M>(&self)
    where
        M: ActonMessage;
}
```

---

## Message Types

### MessageContext<M>

Context passed to message handlers containing the message and metadata.

```rust
pub struct MessageContext<M> {
    // ... internal fields
}

impl<M: ActonMessage> MessageContext<M> {
    /// Get the message payload
    pub fn message(&self) -> &M;

    /// Get mutable access to the message
    pub fn message_mut(&mut self) -> &mut M;

    /// Get the message timestamp
    pub fn timestamp(&self) -> Instant;

    /// Get the reply envelope for responding
    pub fn reply_envelope(&self) -> &OutboundEnvelope;

    /// Send a reply to the sender
    pub fn reply(&self, message: impl ActonMessage);
}
```

---

### OutboundEnvelope

Message wrapper for sending messages with proper addressing.

```rust
pub struct OutboundEnvelope {
    // ... internal fields
}

impl OutboundEnvelope {
    /// Send a message asynchronously
    pub async fn send(&self, message: impl ActonMessage);

    /// Send a message synchronously (for use in sync contexts)
    pub fn reply(&self, message: impl ActonMessage);

    /// Send an Arc-wrapped message (for IPC efficiency)
    pub async fn send_arc(&self, message: Arc<dyn ActonMessage>);

    /// Get the return address
    pub fn return_address(&self) -> &MessageAddress;

    /// Get the recipient address
    pub fn recipient_address(&self) -> &MessageAddress;
}
```

---

### SystemSignal

System-level control signals.

```rust
#[derive(Clone, Debug)]
pub enum SystemSignal {
    /// Request actor shutdown
    Stop,
    /// Actor has stopped
    Stopped,
    /// Custom signal with payload
    Custom(Box<dyn ActonMessage>),
}
```

---

## IPC Types (Feature-Gated)

These types are only available when the `ipc` feature is enabled.

```toml
[dependencies]
acton-reactive = { version = "x.x.x", features = ["ipc"] }
```

### IpcConfig

Configuration for the IPC listener.

```rust
pub struct IpcConfig {
    /// Unix socket path
    pub socket_path: PathBuf,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Connection timeout
    pub connection_timeout: Duration,
    /// Rate limiting configuration
    pub rate_limit: Option<RateLimitConfig>,
}

impl Default for IpcConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from("/tmp/acton.sock"),
            max_connections: 100,
            connection_timeout: Duration::from_secs(30),
            rate_limit: None,
        }
    }
}
```

---

### IpcTypeRegistry

Registry for message types enabling deserialization of IPC messages.

```rust
pub struct IpcTypeRegistry {
    // ... internal fields
}

impl IpcTypeRegistry {
    /// Register a message type with a string identifier
    pub fn register<M>(&self, type_name: &str)
    where
        M: ActonMessage + serde::de::DeserializeOwned;

    /// Check if a type is registered
    pub fn is_registered(&self, type_name: &str) -> bool;

    /// List all registered types
    pub fn registered_types(&self) -> Vec<String>;
}
```

---

### IpcEnvelope

Inbound message envelope from IPC.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcEnvelope {
    /// Target actor identifier
    pub target: String,
    /// Message type name
    pub message_type: String,
    /// Serialized message payload
    pub payload: serde_json::Value,
    /// Optional request ID for correlation
    pub request_id: Option<String>,
    /// Optional reply-to address
    pub reply_to: Option<String>,
}
```

---

### IpcResponse

Response envelope for IPC requests.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    /// Correlation request ID
    pub request_id: Option<String>,
    /// Success indicator
    pub success: bool,
    /// Response payload (if success)
    pub payload: Option<serde_json::Value>,
    /// Error message (if failure)
    pub error: Option<String>,
}
```

---

### IpcError

IPC-specific error type.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcError {
    /// Unknown message type
    UnknownMessageType(String),
    /// Target actor not found
    ActorNotFound(String),
    /// Serialization/deserialization error
    SerializationError(String),
    /// Target actor backpressure
    TargetBusy(String),
    /// Protocol error
    ProtocolError(String),
    /// Rate limited
    RateLimited,
    /// Connection closed
    ConnectionClosed,
    /// Timeout
    Timeout,
}
```

---

### IpcListenerHandle

Handle for managing the IPC listener lifecycle.

```rust
pub struct IpcListenerHandle {
    // ... internal fields
}

impl IpcListenerHandle {
    /// Shutdown the listener gracefully
    pub async fn shutdown(&self) -> anyhow::Result<()>;

    /// Get listener statistics
    pub fn stats(&self) -> IpcListenerStats;

    /// Check if listener is running
    pub fn is_running(&self) -> bool;
}
```

---

### IpcStreamFrame

Frame for streaming responses.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcStreamFrame {
    /// Stream identifier
    pub stream_id: String,
    /// Frame sequence number
    pub sequence: u64,
    /// Frame payload
    pub payload: serde_json::Value,
    /// Is this the final frame?
    pub is_final: bool,
}
```

---

## Configuration

### ActonConfig

System-wide configuration loaded from XDG-compliant locations.

```rust
pub struct ActonConfig {
    pub timeouts: TimeoutConfig,
    pub limits: LimitsConfig,
    pub defaults: DefaultsConfig,
    pub tracing: TracingConfig,
    pub paths: PathsConfig,
    pub behavior: BehaviorConfig,
}
```

See [Configuration Guide](/docs/configuration) for detailed documentation.

---

## Re-exports

### From acton-ern

```rust
pub use acton_ern::prelude::*;
// Includes: Ern, ErnParser, ErnComponent, etc.
```

### From async-trait

```rust
pub use async_trait::async_trait;
```

### From acton-macro

```rust
pub use acton_macro::{acton_message, acton_actor};
```

---

## Feature Flags

| Feature | Description | Dependencies |
|---------|-------------|--------------|
| `default` | No additional features | - |
| `ipc` | Inter-process communication | `mti`, `serde_json`, `parking_lot` |
| `ipc-messagepack` | MessagePack serialization for IPC | `ipc`, `rmp-serde` |

**Example Cargo.toml:**
```toml
[dependencies]
# Basic usage
acton-reactive = "x.x.x"

# With IPC support
acton-reactive = { version = "x.x.x", features = ["ipc"] }

# With IPC and MessagePack
acton-reactive = { version = "x.x.x", features = ["ipc-messagepack"] }
```
