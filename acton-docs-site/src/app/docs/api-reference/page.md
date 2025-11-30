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
| `AgentRuntime` | `common` | Runtime manager for agent system |
| `ManagedAgent` | `actor` | Core agent wrapper with type-state pattern |
| `Idle` | `actor` | Type-state marker for configuration phase |
| `Started` | `actor` | Type-state marker for active agent |
| `AgentHandle` | `common` | External reference to agent |
| `AgentBroker` | `common` | Central pub/sub message broker |
| `AgentConfig` | `actor` | Configuration builder for agents |
| `MessageAddress` | `message` | Agent endpoint address |
| `OutboundEnvelope` | `message` | Message wrapper for sending |
| `BrokerRequest` | `message` | Broadcast message wrapper |
| `AgentReply` | `common` | Utility for handler return types |

### Included Traits

| Trait | Description |
|-------|-------------|
| `ActonMessage` | Marker trait for message payloads |
| `AgentHandleInterface` | Primary trait for agent interaction |
| `Broker` | Message broadcasting capability |
| `Subscriber` | Broker access capability |
| `Subscribable` | Subscription management |

---

## Core Types

### ActonApp

Entry point for initializing the agent system.

```rust
pub struct ActonApp;

impl ActonApp {
    /// Initialize the agent runtime
    pub fn launch() -> AgentRuntime;
}
```

**Example:**
```rust
let runtime = ActonApp::launch();
```

---

### AgentRuntime

Manages the active system and provides methods for creating top-level agents.

```rust
pub struct AgentRuntime {
    pub broker: AgentHandle,
    // ... internal fields
}

impl AgentRuntime {
    /// Create a new agent with default name
    pub fn new_agent<Model>(&mut self) -> ManagedAgent<Idle, Model>
    where
        Model: Default + Clone + Send + Sync + 'static;

    /// Create a new agent with custom name
    pub fn new_agent_with_name<Model>(
        &mut self,
        name: String,
    ) -> ManagedAgent<Idle, Model>
    where
        Model: Default + Clone + Send + Sync + 'static;

    /// Gracefully shutdown all agents
    pub async fn shutdown_all(&mut self) -> anyhow::Result<()>;

    /// Get the system broker
    pub fn get_broker(&self) -> &AgentHandle;
}
```

**Example:**
```rust
let mut runtime = ActonApp::launch();
let mut agent = runtime.new_agent::<MyState>();
// ... configure and start agent
runtime.shutdown_all().await?;
```

---

### ManagedAgent<AgentState, Model>

Core agent wrapper using the type-state pattern to enforce valid operations at compile time.

#### Type Parameters

- `AgentState`: Either `Idle` or `Started`
- `Model`: User-defined state type (must implement `Default + Clone + Send + Sync + 'static`)

#### Methods (Idle State)

```rust
impl<Model> ManagedAgent<Idle, Model> {
    /// Register a mutable message handler
    pub fn mutate_on<M>(
        &mut self,
        handler: impl Fn(&mut ManagedAgent<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register a read-only message handler (concurrent execution)
    pub fn act_on<M>(
        &mut self,
        handler: impl Fn(&ManagedAgent<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register a fallible mutable handler
    pub fn mutate_on_fallible<M>(
        &mut self,
        handler: impl Fn(&mut ManagedAgent<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = Result<Box<dyn ActonMessageReply>, Box<dyn Error>>>
            + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register a fallible read-only handler
    pub fn act_on_fallible<M>(
        &mut self,
        handler: impl Fn(&ManagedAgent<Started, Model>, &mut MessageContext<M>)
            -> impl Future<Output = Result<Box<dyn ActonMessageReply>, Box<dyn Error>>>
            + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage;

    /// Register an error handler for a specific message and error type
    pub fn on_error<M, E>(
        &mut self,
        handler: impl Fn(&mut ManagedAgent<Started, Model>, &mut MessageContext<M>, &E)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self
    where
        M: ActonMessage,
        E: Error + 'static;

    /// Lifecycle hook: runs before message loop starts
    pub fn before_start(
        &mut self,
        hook: impl Fn(&ManagedAgent<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Lifecycle hook: runs after message loop starts
    pub fn after_start(
        &mut self,
        hook: impl Fn(&ManagedAgent<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Lifecycle hook: runs before message loop stops
    pub fn before_stop(
        &mut self,
        hook: impl Fn(&ManagedAgent<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Lifecycle hook: runs after message loop stops
    pub fn after_stop(
        &mut self,
        hook: impl Fn(&ManagedAgent<Started, Model>)
            -> impl Future<Output = ()> + Send + Sync + 'static,
    ) -> &mut Self;

    /// Start the agent, transitioning from Idle to Started
    pub async fn start(self) -> AgentHandle;
}
```

#### Methods (Started State)

```rust
impl<Model> ManagedAgent<Started, Model> {
    /// Get the agent's state
    pub fn model(&self) -> &Model;

    /// Get mutable access to the agent's state
    pub fn model_mut(&mut self) -> &mut Model;

    /// Create a new outbound envelope for sending messages
    pub fn new_envelope(&self) -> OutboundEnvelope;

    /// Get the agent's handle
    pub fn handle(&self) -> &AgentHandle;

    /// Access supervised children
    pub fn children(&self) -> &DashMap<String, AgentHandle>;

    /// Supervise a new child agent
    pub async fn supervise<ChildModel>(
        &self,
        child: ManagedAgent<Idle, ChildModel>,
    ) -> anyhow::Result<AgentHandle>
    where
        ChildModel: Default + Clone + Send + Sync + 'static;

    /// Find a child by Ern
    pub fn find_child(&self, id: &Ern) -> Option<AgentHandle>;
}
```

**Example:**
```rust
#[derive(Default, Clone)]
struct CounterState {
    count: u32,
}

#[derive(Clone, Debug)]
struct Increment(u32);

let mut agent = runtime.new_agent::<CounterState>();

agent
    .mutate_on::<Increment>(|agent, ctx| {
        Box::pin(async move {
            agent.model_mut().count += ctx.message().0;
        })
    })
    .before_start(|agent| {
        Box::pin(async move {
            println!("Agent starting with count: {}", agent.model().count);
        })
    });

let handle = agent.start().await;
```

---

### AgentHandle

External reference to an agent for sending messages and managing lifecycle.

```rust
pub struct AgentHandle {
    // ... internal fields
}

impl AgentHandle {
    /// Send a message to this agent
    pub async fn send(&self, message: impl ActonMessage);

    /// Send a message synchronously (spawns runtime if needed)
    pub fn send_sync(&self, message: impl ActonMessage, recipient: &MessageAddress);

    /// Stop the agent gracefully
    pub async fn stop(&self) -> anyhow::Result<()>;

    /// Get the agent's Ern identifier
    pub fn id(&self) -> Ern;

    /// Get the agent's name
    pub fn name(&self) -> String;

    /// Get the agent's reply address
    pub fn reply_address(&self) -> MessageAddress;

    /// Create an envelope for sending to a specific recipient
    pub fn create_envelope(&self, recipient: &MessageAddress) -> OutboundEnvelope;

    /// Access supervised children
    pub fn children(&self) -> &DashMap<String, AgentHandle>;

    /// Find a child by Ern
    pub fn find_child(&self, id: &Ern) -> Option<AgentHandle>;

    /// Clone the handle reference
    pub fn clone_ref(&self) -> Self;
}
```

---

### AgentBroker

Central pub/sub message broker for broadcasting messages to subscribers.

```rust
pub struct AgentBroker {
    // Derefs to AgentHandle
}

impl Deref for AgentBroker {
    type Target = AgentHandle;
}

impl AgentBroker {
    /// Broadcast a message to all subscribers of that message type
    pub async fn broadcast(&self, message: impl ActonMessage);

    /// Broadcast synchronously
    pub fn broadcast_sync(&self, message: impl ActonMessage) -> anyhow::Result<()>;
}
```

---

## Traits

### ActonMessage

Marker trait for types that can be sent as messages between agents.

```rust
pub trait ActonMessage: DynClone + Any + Send + Sync + Debug {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
```

**Automatic Implementation:**

Any type implementing `Clone + Debug + Send + Sync + 'static` automatically implements `ActonMessage`.

**Example:**
```rust
#[derive(Clone, Debug)]
struct MyMessage {
    data: String,
}
// MyMessage automatically implements ActonMessage
```

---

### AgentHandleInterface

Primary trait for agent interaction, implemented by `AgentHandle` and `AgentBroker`.

```rust
pub trait AgentHandleInterface: Clone + Debug + Send + Sync + 'static {
    /// Get the agent's reply address
    fn reply_address(&self) -> MessageAddress;

    /// Send a message to the agent
    fn send(&self, message: impl ActonMessage) -> impl Future<Output = ()> + Send + Sync;

    /// Send synchronously
    fn send_sync(&self, message: impl ActonMessage, recipient: &MessageAddress);

    /// Stop the agent
    fn stop(&self) -> impl Future<Output = anyhow::Result<()>> + Send + Sync;

    /// Access children
    fn children(&self) -> &DashMap<String, AgentHandle>;

    /// Find a child
    fn find_child(&self, id: &Ern) -> Option<AgentHandle>;

    /// Get agent identifier
    fn id(&self) -> Ern;

    /// Get agent name
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
        Self: AgentHandleInterface + Sized;
}
```

---

### Subscribable

Trait for managing pub/sub subscriptions.

```rust
pub trait Subscribable: AgentHandleInterface + Subscriber {
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
    /// Request agent shutdown
    Stop,
    /// Agent has stopped
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
    /// Target agent identifier
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
    /// Target agent not found
    AgentNotFound(String),
    /// Serialization/deserialization error
    SerializationError(String),
    /// Target agent backpressure
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
