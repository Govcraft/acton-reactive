---
title: Architecture
nextjs:
  metadata:
    title: Architecture - acton-reactive
    description: Comprehensive architectural documentation for the acton-reactive crate, including system diagrams, component relationships, and design patterns.
---

This document provides comprehensive architectural documentation for the `acton-reactive` crate, including system diagrams, component relationships, and design patterns.

---

## High-Level Architecture

```mermaid
graph TB
    subgraph "Application Layer"
        App[User Application]
    end

    subgraph "acton-reactive Core"
        ActonApp[ActonApp]
        Runtime[AgentRuntime]
        Broker[AgentBroker]
    end

    subgraph "Agent System"
        Agent1[ManagedAgent 1]
        Agent2[ManagedAgent 2]
        Agent3[ManagedAgent 3]
    end

    subgraph "Messaging Layer"
        Handlers[Message Handlers]
        Envelopes[Envelopes]
        Channels[MPSC Channels]
    end

    subgraph "External"
        IPC[IPC Listener]
        ExtProc[External Processes]
    end

    App --> ActonApp
    ActonApp --> Runtime
    Runtime --> Broker
    Runtime --> Agent1
    Runtime --> Agent2
    Runtime --> Agent3

    Agent1 <--> Channels
    Agent2 <--> Channels
    Agent3 <--> Channels

    Channels --> Envelopes
    Envelopes --> Handlers

    Broker <-.-> Agent1
    Broker <-.-> Agent2
    Broker <-.-> Agent3

    IPC --> Runtime
    ExtProc --> IPC
```

### Core Components

| Component | Responsibility |
|-----------|----------------|
| **ActonApp** | Entry point; initializes the runtime |
| **AgentRuntime** | Manages agent lifecycle and system state |
| **AgentBroker** | Central pub/sub message broker |
| **ManagedAgent** | Individual agent with state and handlers |
| **AgentHandle** | External reference for agent interaction |

---

## Module Structure

```mermaid
graph LR
    subgraph "lib.rs Prelude"
        Prelude[Public API Surface]
    end

    subgraph "actor/"
        ManagedAgent[managed_agent.rs]
        AgentConfig[agent_config.rs]
        Idle[idle.rs]
        Started[started.rs]
    end

    subgraph "common/"
        Acton[acton.rs]
        AgentRuntime2[agent_runtime.rs]
        AgentHandle2[agent_handle.rs]
        AgentBroker2[agent_broker.rs]
        Config[config.rs]
        IPCMod[ipc/]
    end

    subgraph "message/"
        Envelope[envelope.rs]
        OutboundEnvelope[outbound_envelope.rs]
        MessageAddress[message_address.rs]
        MessageContext[message_context.rs]
        BrokerRequest[broker_request.rs]
    end

    subgraph "traits/"
        ActonMessage[acton_message.rs]
        AgentHandleInterface[agent_handle_interface.rs]
        BrokerTrait[broker.rs]
        Subscriber[subscriber.rs]
    end

    Prelude --> ManagedAgent
    Prelude --> Acton
    Prelude --> Envelope
    Prelude --> ActonMessage
```

### Directory Layout

```text
acton-reactive/src/
├── lib.rs                    # Public API & prelude
├── actor/
│   ├── mod.rs
│   ├── managed_agent.rs      # Core ManagedAgent type
│   ├── agent_config.rs       # AgentConfig builder
│   └── managed_agent/
│       ├── idle.rs           # Idle state methods
│       └── started.rs        # Started state methods
├── common/
│   ├── mod.rs
│   ├── acton.rs              # ActonApp entry point
│   ├── acton_inner.rs        # Internal runtime state
│   ├── agent_runtime.rs      # AgentRuntime
│   ├── agent_handle.rs       # AgentHandle
│   ├── agent_broker.rs       # AgentBroker
│   ├── agent_reply.rs        # AgentReply utility
│   ├── config.rs             # ActonConfig
│   ├── types.rs              # Internal type aliases
│   └── ipc/                  # IPC module (feature-gated)
│       ├── mod.rs
│       ├── config.rs
│       ├── listener.rs
│       ├── protocol.rs
│       ├── registry.rs
│       ├── types.rs
│       ├── rate_limiter.rs
│       └── subscription_manager.rs
├── message/
│   ├── mod.rs
│   ├── envelope.rs           # Internal Envelope
│   ├── outbound_envelope.rs  # OutboundEnvelope
│   ├── message_address.rs    # MessageAddress
│   ├── message_context.rs    # MessageContext<M>
│   ├── broker_request.rs     # BrokerRequest
│   ├── broker_request_envelope.rs
│   ├── signal.rs             # SystemSignal
│   ├── message_error.rs      # MessageError
│   ├── subscribe_broker.rs   # SubscribeBroker
│   └── unsubscribe_broker.rs # UnsubscribeBroker
└── traits/
    ├── mod.rs
    ├── acton_message.rs      # ActonMessage trait
    ├── acton_message_reply.rs
    ├── agent_handle_interface.rs
    ├── broker.rs             # Broker trait
    ├── subscriber.rs         # Subscriber trait
    └── subscribable.rs       # Subscribable trait
```

---

## Agent Lifecycle

```mermaid
stateDiagram-v2
    [*] --> Created: new_agent
    Created --> Idle: Configuration Phase

    state Idle {
        [*] --> Configuring
        Configuring --> Configuring: mutate_on
        Configuring --> Configuring: act_on
        Configuring --> Configuring: before_start
        Configuring --> Configuring: after_start
        Configuring --> Configuring: before_stop
        Configuring --> Configuring: after_stop
    }

    Idle --> Starting: start await

    state Starting {
        [*] --> CreatingChannel
        CreatingChannel --> SpawningTask
        SpawningTask --> RunningBeforeStart
        RunningBeforeStart --> EnteringLoop
    }

    Starting --> Started: Message Loop Active

    state Started {
        [*] --> WaitingForMessage
        WaitingForMessage --> ProcessingMessage: Message Received
        ProcessingMessage --> DispatchingHandler
        DispatchingHandler --> ExecutingHandler
        ExecutingHandler --> WaitingForMessage: Handler Complete
    }

    Started --> Stopping: stop or SystemSignal Stop

    state Stopping {
        [*] --> ExitingLoop
        ExitingLoop --> RunningBeforeStop
        RunningBeforeStop --> ClosingChannel
        ClosingChannel --> RunningAfterStop
    }

    Stopping --> [*]: Agent Terminated
```

### Lifecycle Hooks

```mermaid
sequenceDiagram
    participant User
    participant Agent as ManagedAgent
    participant Runtime as Tokio Runtime
    participant Loop as Message Loop

    User->>Agent: start await
    Agent->>Runtime: spawn task
    Runtime->>Agent: before_start
    Agent->>Loop: Enter message loop
    Runtime->>Agent: after_start

    Note over Loop: Processing messages...

    User->>Agent: stop
    Loop->>Agent: Exit message loop
    Runtime->>Agent: before_stop
    Runtime->>Agent: Close channels
    Runtime->>Agent: after_stop
    Agent->>User: Agent stopped
```

---

## Message Flow

### Direct Messaging

```mermaid
sequenceDiagram
    participant Sender as Sender Agent
    participant Handle as AgentHandle
    participant Envelope as OutboundEnvelope
    participant Channel as MPSC Channel
    participant Receiver as Receiver Agent
    participant Handler as Message Handler

    Sender->>Handle: handle.send message
    Handle->>Envelope: create envelope
    Envelope->>Envelope: wrap in Arc
    Envelope->>Channel: send to channel
    Channel->>Receiver: deliver envelope
    Receiver->>Receiver: lookup handler by TypeId
    Receiver->>Handler: downcast and dispatch
    Handler->>Receiver: handler executes
```

### Handler Types

```mermaid
graph TB
    subgraph "Handler Registration"
        MutateOn[mutate_on M]
        ActOn[act_on M]
        MutateOnFallible[mutate_on_fallible M]
        ActOnFallible[act_on_fallible M]
    end

    subgraph "Handler Storage"
        MutableHandlers[message_handlers: DashMap]
        ReadOnlyHandlers[read_only_handlers: DashMap]
        ErrorHandlers[error_handlers: DashMap]
    end

    subgraph "Execution Model"
        Exclusive[Exclusive Access Sequential]
        Concurrent[Shared Access Concurrent with HWM]
    end

    MutateOn --> MutableHandlers
    MutateOnFallible --> MutableHandlers
    ActOn --> ReadOnlyHandlers
    ActOnFallible --> ReadOnlyHandlers

    MutableHandlers --> Exclusive
    ReadOnlyHandlers --> Concurrent
```

---

## Pub/Sub Architecture

```mermaid
graph TB
    subgraph "Publishers"
        Pub1[Agent 1]
        Pub2[Agent 2]
    end

    subgraph "AgentBroker"
        BrokerAgent[Broker Agent]
        SubRegistry[Subscription Registry TypeId to Vec MessageAddress]
    end

    subgraph "Subscribers"
        Sub1[Agent A]
        Sub2[Agent B]
        Sub3[Agent C]
    end

    Pub1 -->|broadcast msg| BrokerAgent
    Pub2 -->|broadcast msg| BrokerAgent

    BrokerAgent --> SubRegistry
    SubRegistry -->|lookup TypeId| Sub1
    SubRegistry -->|lookup TypeId| Sub2
    SubRegistry -->|lookup TypeId| Sub3

    Sub1 -.->|subscribe M| SubRegistry
    Sub2 -.->|subscribe M| SubRegistry
    Sub3 -.->|subscribe M| SubRegistry
```

### Subscription Flow

```mermaid
sequenceDiagram
    participant Agent
    participant Broker as AgentBroker
    participant Registry as Subscription Registry

    Agent->>Broker: subscribe PriceUpdate
    Broker->>Registry: register TypeId of PriceUpdate agent_address
    Registry-->>Broker: OK
    Broker-->>Agent: Subscribed

    Note over Agent,Registry: Later...

    participant Publisher
    Publisher->>Broker: broadcast PriceUpdate
    Broker->>Registry: lookup subscribers for TypeId
    Registry-->>Broker: agent_address list
    Broker->>Agent: forward PriceUpdate
```

---

## Supervision Hierarchy

```mermaid
graph TB
    subgraph "Root Level"
        Runtime[AgentRuntime]
        Roots[roots: DashMap]
    end

    subgraph "First Level Agents"
        Parent1[Parent Agent 1]
        Parent2[Parent Agent 2]
    end

    subgraph "Child Agents"
        Child1A[Child 1A]
        Child1B[Child 1B]
        Child2A[Child 2A]
    end

    subgraph "Grandchild Agents"
        GC1[Grandchild 1]
    end

    Runtime --> Roots
    Roots --> Parent1
    Roots --> Parent2

    Parent1 -->|supervise| Child1A
    Parent1 -->|supervise| Child1B
    Parent2 -->|supervise| Child2A

    Child1A -->|supervise| GC1
```

### ERN Hierarchy

```text
root_service/                    # Root agent ERN
├── root_service/worker_1        # Child agent ERN
├── root_service/worker_2        # Child agent ERN
│   └── root_service/worker_2/validator   # Grandchild ERN
└── root_service/worker_3        # Child agent ERN
```

---

## IPC Architecture

```mermaid
graph TB
    subgraph "External Processes"
        Client1[Python Client]
        Client2[Node.js Client]
        Client3[Other Process]
    end

    subgraph "IPC Layer"
        Socket[Unix Socket /tmp/acton.sock]
        Listener[IpcListener]
        Protocol[Wire Protocol Length-Prefixed Frames]
        RateLimiter[Rate Limiter]
    end

    subgraph "Type System"
        Registry[IpcTypeRegistry]
        Deserializer[Message Deserializer]
    end

    subgraph "Agent System"
        Router[Agent Router]
        Agent1[Agent 1]
        Agent2[Agent 2]
        BrokerNode[AgentBroker]
    end

    Client1 --> Socket
    Client2 --> Socket
    Client3 --> Socket

    Socket --> Listener
    Listener --> RateLimiter
    RateLimiter --> Protocol
    Protocol --> Registry
    Registry --> Deserializer
    Deserializer --> Router

    Router --> Agent1
    Router --> Agent2
    Router --> BrokerNode
```

---

## Design Patterns

### Type-State Pattern

```mermaid
graph LR
    subgraph "Compile-Time Enforcement"
        Idle[ManagedAgent Idle M]
        Started[ManagedAgent Started M]
    end

    subgraph "Idle-Only Methods"
        MutateOn[mutate_on]
        ActOn[act_on]
        Hooks[lifecycle hooks]
    end

    subgraph "Started-Only Methods"
        Model[model]
        ModelMut[model_mut]
        Send[send]
        Supervise[supervise]
    end

    subgraph "Transition"
        Start[start await]
    end

    MutateOn --> Idle
    ActOn --> Idle
    Hooks --> Idle

    Idle -->|start| Start
    Start --> Started

    Model --> Started
    ModelMut --> Started
    Send --> Started
    Supervise --> Started
```

---

## Key Architectural Decisions

### 1. Type-State for Lifecycle Safety

The use of phantom type parameters (`Idle`/`Started`) ensures that configuration methods are only available before an agent starts, and runtime methods are only available after. This prevents entire classes of bugs at compile time.

### 2. Arc-Based Message Sharing

Messages are wrapped in `Arc<dyn ActonMessage>` for efficient broadcast distribution. When a message is broadcast to N subscribers, only one allocation occurs, and N Arc clones are created (cheap reference count increments).

### 3. Handler Concurrency Model

- **Mutable handlers** (`mutate_on`): Execute with exclusive access, one at a time
- **Read-only handlers** (`act_on`): Execute concurrently with high-water mark control

This allows maximum concurrency for read operations while maintaining safety for mutations.

### 4. Channel-Based Communication

All inter-agent communication uses Tokio MPSC channels, providing:
- Backpressure handling
- Non-blocking sends
- Proper ordering guarantees

### 5. XDG-Compliant Configuration

Configuration follows the XDG Base Directory Specification for cross-platform compatibility:
- Linux: `~/.config/acton/config.toml`
- macOS: `~/Library/Application Support/acton/config.toml`
- Windows: `%APPDATA%/acton/config.toml`

### 6. Feature-Gated IPC

IPC functionality is optional via feature flags, keeping the core library lean for applications that don't need external process communication.

---

## Runtime Initialization Sequence

```mermaid
sequenceDiagram
    participant User
    participant ActonApp
    participant Config as ActonConfig
    participant Inner as ActonInner
    participant Runtime as AgentRuntime
    participant Broker as AgentBroker

    User->>ActonApp: launch
    ActonApp->>Config: load from XDG
    Config-->>ActonApp: ActonConfig

    ActonApp->>Inner: new
    Inner->>Inner: Create cancellation_token
    Inner->>Broker: Create broker agent
    Broker-->>Inner: AgentHandle

    opt IPC Feature Enabled
        Inner->>Inner: Create IpcTypeRegistry
    end

    Inner-->>ActonApp: ActonInner
    ActonApp->>Runtime: new inner
    Runtime-->>User: AgentRuntime
```

---

## Shutdown Sequence

```mermaid
sequenceDiagram
    participant User
    participant Runtime as AgentRuntime
    participant Token as CancellationToken
    participant Roots as Root Agents
    participant Children as Child Agents
    participant Tracker as TaskTracker

    User->>Runtime: shutdown_all
    Runtime->>Token: cancel

    loop For each root agent
        Runtime->>Roots: stop
        Roots->>Children: propagate stop
        Children-->>Roots: stopped
        Roots-->>Runtime: stopped
    end

    Runtime->>Tracker: wait
    Tracker-->>Runtime: all tasks complete
    Runtime-->>User: Ok
```
