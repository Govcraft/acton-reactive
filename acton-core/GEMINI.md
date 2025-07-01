# Gemini Code-Assist Workspace Configuration for `acton-core`

This document provides a guide for understanding and contributing to the `acton-core` crate. It outlines the project's architecture, key concepts, naming conventions, and development workflow.

## 1. Project Overview

`acton-core` is the foundational crate for the Acton Reactive Application Framework. It provides the low-level components for an asynchronous, message-passing agent system built on Tokio. It is not intended for direct use by end-users but serves as the engine for the higher-level `acton-reactive` crate.

### Key Architectural Principles:

- **Agent-Based Concurrency:** Logic is encapsulated in independent "Agents" that manage their own state.
- **Asynchronous Messaging:** Agents communicate by sending immutable messages through asynchronous channels.
- **Supervision:** Agents can create and manage child agents, forming a hierarchy.
- **Publish/Subscribe:** A central "Broker" agent allows for decoupled, system-wide message broadcasting.
- **Trait-Driven Design:** Core functionalities are defined by traits (`AgentHandleInterface`, `Broker`, `ActonMessage`) for abstraction and composability.

## 2. Core Concepts & Naming Conventions

The project deliberately uses specific, approachable terminology in its public-facing APIs to lower the barrier to entry for developers new to actor systems. Adhering to these conventions is critical.

| Concept | Public Name | Internal/Implementation Detail | Description |
| :--- | :--- | :--- | :--- |
| **The Actor** | **Agent** | `ManagedAgent<S>` | The fundamental unit of computation. Consists of user-defined state (`model`) wrapped in the framework's `ManagedAgent` struct, which handles the lifecycle. |
| **Actor Handle**| **`AgentHandle`** | `AgentHandle` | An inexpensive, cloneable reference used to interact with an Agent (send messages, stop it, etc.). This is the primary public API for agent interaction. |
| **Actor Trait** | **`AgentHandleInterface`** | `Actor` (old name) | The trait defining the standard operations available on an `AgentHandle`. The name `Actor` should be avoided in public APIs. |
| **Identifier** | **`id`** | `Ern` | The unique identifier for an agent. Publicly accessed via `handle.id()`. Internally, it's an "Entity Resource Name" (`Ern`), but this is an implementation detail. |
| **Message** | **Message** | `ActonMessage` | A simple struct or enum that implements `Debug` and `Clone`. The `ActonMessage` trait is the marker trait for all messages. |
| **Handler** | **`act_on` / `act_on_fallible`** | `Reactor` | A function registered on an agent builder that defines how the agent reacts to a specific message type. Internally, these are called "reactors". |
| **Runtime** | **`AgentRuntime`** | `ActonInner` | The system environment that manages agents. It's created via `ActonApp::launch()`. |
| **Pub/Sub** | **`AgentBroker`** | `AgentBroker` | The central agent responsible for broadcasting messages to all subscribed agents. Accessed via `runtime.broker()` or `agent.broker()`. |

## 3. Development Workflow

### Project Structure

- `src/actor/`: Defines `ManagedAgent` (the agent state machine) and `AgentConfig`.
- `src/common/`: Contains the runtime (`AgentRuntime`), entry point (`ActonApp`), interaction handle (`AgentHandle`), and the message broker (`AgentBroker`).
- `src/message/`: Defines message wrappers (`Envelope`, `BrokerRequest`) and system signals.
- `src/traits/`: Defines the core interfaces (`AgentHandleInterface`, `ActonMessage`, `Broker`, `Subscribable`).

### Architectural Deep Dive

To get up to speed on the architecture, reading the following files in order is recommended:

1.  **`src/lib.rs`**: Understand the overall module structure and the public `prelude`.
2.  **`src/traits/mod.rs` and its contents**: The traits define the core contracts of the framework. `AgentHandleInterface` and `ActonMessage` are especially important.
3.  **`src/actor/managed_agent.rs`**: This is the heart of an agent. Pay attention to the `ManagedAgent` struct and its type-state pattern (`Idle`, `Started`).
4.  **`src/actor/managed_agent/idle.rs`**: Shows how an agent is configured (`act_on`, lifecycle hooks) before it starts.
5.  **`src/actor/managed_agent/started.rs`**: Shows the agent's main event loop (`wake`) and how messages are processed.
6.  **`src/common/agent_handle.rs`**: This is the public-facing interface for an agent. Understand how it sends messages and manages children.
7.  **`src/common/agent_runtime.rs`**: Explains how the system is bootstrapped and how top-level agents are created.
8.  **`src/common/agent_broker.rs`**: Details the implementation of the publish-subscribe system.

### Creating and Running an Agent

The standard lifecycle for an agent is:
1.  **Define State:** Create a struct for the agent's state. The `#[acton_actor]` macro is the preferred way to derive the necessary `Default` and `Debug` traits.
    ```rust
    use acton_macro::acton_actor;

    #[acton_actor]
    struct MyAgentState { count: i32 }
    ```
2.  **Define Messages:** Create structs or enums for messages. The `#[acton_message]` macro is the preferred way to derive the necessary `Clone` and `Debug` traits and ensure it implements `ActonMessage`.
    ```rust
    use acton_macro::acton_message;

    #[acton_message]
    struct MyMessage;
    ```
3.  **Get a Builder:** Use `runtime.new_agent::<MyAgentState>().await` to get an agent builder (`ManagedAgent<Idle, ...>`).
4.  **Configure Handlers:** Use `.act_on::<MyMessage>(...)` on the builder to define how the agent reacts to messages. Handlers can be synchronous or asynchronous.
    - For fallible operations, use `.act_on_fallible<...>()` and register a corresponding `.on_error<...>()` handler.
5.  **Start the Agent:** Call `.start().await` on the builder. This consumes the builder and returns an `AgentHandle`.

### Ergonomic API Examples

The following tests in `acton-reactive/tests/` serve as excellent examples of the intended public API ergonomics. They demonstrate how users should interact with the framework.

-   **`direct_messaging_tests.rs`**: The `test_reply` function shows the ideal pattern for request-reply interactions between two agents. It exemplifies creating agents, passing a handle from one to the other, and using message handlers to communicate directly.
-   **`broker_tests.rs`**: The `test_broker` and `test_broker_from_handler` functions demonstrate the publish-subscribe pattern. They show how to subscribe agents to message types and how to broadcast messages from the main runtime or from within another agent's handler.
-   **`actor_tests.rs`**:
    -   `test_child_actor`: A clear example of creating a child agent and having a parent `supervise` it.
    -   `test_actor_mutation`: Shows the fundamental pattern of an agent mutating its own state in response to a message.
-   **`result_error_handler_tests.rs`**: The `test_result_and_error_handler_fires` function shows the modern, preferred way of handling fallible operations within an agent using `act_on_fallible` and `on_error`.

### Testing

- **Test Location:** All integration tests are located in the `acton-reactive/tests/` directory. This project serves as the testing harness for `acton-core`.
- **Test Macro:** Use the `#[acton_test]` macro for setting up the Tokio runtime.
- **Test Pattern:**
    1.  Call `initialize_tracing()` for logging.
    2.  Launch the runtime: `let mut runtime = ActonApp::launch();`.
    3.  Create and configure agent builders as described above.
    4.  Subscribe agents to messages if using the broker: `builder.handle().subscribe::<MyMessage>().await;`.
    5.  Start the agents to get their handles: `let handle = builder.start().await;`.
    6.  Send messages to the handles to trigger behavior: `handle.send(MyMessage).await;`.
    7.  Use `after_stop` lifecycle handlers on the agent builders to assert the final state of the agent's model.
    8.  Shut down the system gracefully: `runtime.shutdown_all().await?;`.
