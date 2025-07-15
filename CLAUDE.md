# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Acton Reactive** is a Rust framework for building fast, concurrent applications using an actor model. It provides a message-passing architecture where independent "agents" communicate via asynchronous messages, built on top of Tokio.

## Workspace Structure

This is a Cargo workspace with 4 primary crates:

- **acton-core**: Foundation with core traits, types, and runtime management
- **acton-macro**: Procedural macros for deriving message traits
- **acton-reactive**: Main user-facing API with high-level abstractions
- **acton-test**: Testing utilities and macros
- **acton-cli**: Command-line tool for project scaffolding

## Development Commands

### Build & Check
```bash
cargo check --workspace              # Check all crates compile
cargo build --workspace             # Build all crates
cargo build --release               # Optimized build
```

### Testing
```bash
cargo test                          # Run all tests
cargo test --workspace              # Run tests for all workspace members
cargo test --test <test_name>       # Run specific test file
cargo test actor_tests              # Run specific test module
```

### Examples
```bash
cargo run --example basic          # Run basic counter example
cargo run --example broadcast      # Run pub/sub messaging example
cargo run --example fruit_market   # Run marketplace simulation
cargo run --example lifecycles     # Run agent lifecycle example
```

### Publishing (CI/CD)
```bash
make publish-all                   # Publish all crates in dependency order
make publish-force                 # Force publish regardless of changes
make prepare-release              # Prepare Cargo.toml for release
```

## Core Architecture

### Key Concepts
- **Agent**: Independent computational unit with internal state (`ManagedAgent`)
- **Message**: Communication unit between agents (must implement `ActonMessage`)
- **Handle**: Cloneable reference to interact with agents (`AgentHandle`)
- **Broker**: Central pub/sub system for message distribution (`AgentBroker`)
- **Runtime**: System management via `ActonApp::launch()`

### Message Flow
1. Agents define message handlers using `.act_on::<MessageType>()`
2. Messages sent via `AgentHandle::send().await`
3. Broker handles pub/sub via `subscribe()` and `broadcast()`
4. Lifecycle hooks: `.before_start()`, `.after_start()`, `.before_stop()`, `.after_stop()`

### Type State Pattern
Agents transition through states:
- `Idle` → `Started` (via `.start().await`)
- Provides compile-time safety for agent lifecycle

## Common Tasks

### Creating a New Agent
```rust
use acton_reactive::prelude::*;

#[derive(Debug, Default)]
struct MyAgent { count: i32 }

#[acton_message]
struct Increment;

// In async context:
let mut app = ActonApp::launch();
let mut builder = app.new_agent::<MyAgent>().await;
builder.act_on::<Increment>(|agent, _| {
    agent.model.count += 1;
    AgentReply::immediate()
});
let handle = builder.start().await;
handle.send(Increment).await;
```

### Testing Patterns
Tests use `acton_test` utilities and follow patterns in:
- `acton-reactive/tests/` - Integration tests
- Test setup in `tests/setup/` - Common test actors and messages

### Project Scaffolding
Use acton-cli for new projects:
```bash
cd acton-cli
cargo run -- new my_project    # Create new project from templates
```

## File Organization

- **Core traits**: `acton-core/src/traits/`
- **Message types**: `acton-core/src/message/`
- **Agent implementation**: `acton-core/src/actor/`
- **Examples**: `acton-reactive/examples/`
- **Test utilities**: `acton-test/src/`
- **CLI templates**: `acton-cli/src/templates/`

## Dependencies

Built on Tokio async runtime with these key dependencies:
- `tokio` - Async runtime
- `tracing` - Logging and instrumentation
- `async-trait` - Async trait support
- `anyhow` - Error handling