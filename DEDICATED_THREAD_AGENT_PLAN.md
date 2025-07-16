# Acton Reactive v6.0.0: Dedicated Thread Agents

## Overview

This feature introduces the ability to designate specific agents to run on their own dedicated OS thread, separate from the main Tokio runtime thread pool. This is particularly useful for agents that manage I/O resources (like terminal/console access) or require thread-local state, such as the Printer agent in the fruit market example.

## Motivation

The current Acton Reactive architecture runs all agents as lightweight Tokio tasks within a shared thread pool. While this provides excellent scalability for CPU-bound work, it presents challenges for agents that:

1. **Manage exclusive I/O resources** (e.g., terminal/console access, serial ports)
2. **Use thread-local storage** or thread-specific APIs
3. **Require consistent thread affinity** for performance or correctness
4. **Need to block synchronously** without affecting the main runtime

The fruit market's Printer agent exemplifies this - it manages terminal state and would benefit from running on a dedicated thread to avoid conflicts and ensure consistent terminal access.

## Feature Description

### Dedicated Thread Agent Pattern

A new agent configuration option that allows specifying an agent should run on its own dedicated OS thread, with full integration into the Acton message-passing system.

### Key Characteristics

1. **Dedicated OS Thread**: Each designated agent gets its own thread
2. **Message Compatibility**: Full compatibility with existing Acton message system
3. **Lifecycle Integration**: Same lifecycle hooks and supervision as regular agents
4. **Performance**: Suitable for I/O-bound agents, not CPU-intensive work
5. **Backward Compatibility**: Existing agents continue to work unchanged

## API Design

### Configuration-Based Approach (Recommended)

```rust
// New configuration enum for agent threading model
#[derive(Debug, Clone, Copy, Default)]
pub enum AgentThreading {
    #[default]
    Default,      // Uses Tokio thread pool (current behavior)
    Dedicated,    // Runs on dedicated OS thread
}

// Extended AgentConfig with threading option
impl AgentConfig {
    pub fn with_threading(mut self, threading: AgentThreading) -> Self {
        self.threading = threading;
        self
    }
}

// Usage in agent creation
let mut printer_builder = runtime
    .new_agent_with_config::<Printer>(
        AgentConfig::new("printer")
            .with_threading(AgentThreading::Dedicated)
    ).await;
```

### Alternative: Builder Method Approach

```rust
// Alternative fluent API
let mut printer_builder = runtime.new_agent::<Printer>().await;
printer_builder.dedicated_thread(); // New method
```

## Implementation Plan

### Phase 1: Type System Extensions
- [ ] Add `AgentThreading` enum to `acton-core/src/common/types.rs`
- [ ] Extend `AgentConfig` with threading field
- [ ] Add validation for threading configuration

### Phase 2: Runtime Integration
- [ ] Modify `AgentRuntime::spawn_agent` to handle dedicated thread spawning
- [ ] Create new dedicated thread spawning mechanism using `std::thread`
- [ ] Implement message channel bridging between dedicated thread and main runtime

### Phase 3: Message Dispatch Adaptation
- [ ] Modify `ManagedAgent::wake()` to detect dedicated thread agents
- [ ] Create dedicated thread event loop for message processing
- [ ] Implement proper shutdown signaling for dedicated threads

### Phase 4: Testing & Examples
- [ ] Create comprehensive test suite for dedicated thread agents
- [ ] Update fruit market example to use dedicated thread for Printer
- [ ] Add new examples showcasing dedicated thread patterns
- [ ] Performance benchmarks comparing thread models

### Phase 5: Documentation
- [ ] Update API documentation with threading options
- [ ] Create migration guide for existing agents
- [ ] Add best practices documentation for when to use dedicated threads

## Architecture Details

### Dedicated Thread Implementation

```rust
// Core structure for dedicated thread agents
struct DedicatedThreadAgent<M> {
    agent: ManagedAgent<Started, M>,
    message_tx: Sender<Box<dyn ActonMessage>>,
    shutdown_token: CancellationToken,
}

impl<M> DedicatedThreadAgent<M> {
    fn spawn(mut agent: ManagedAgent<Started, M>) -> AgentHandle {
        let (message_tx, message_rx) = mpsc::channel(32);
        let shutdown_token = CancellationToken::new();
        let handle = agent.handle().clone();
        
        // Spawn OS thread
        std::thread::spawn(move || {
            self::dedicated_thread_loop(agent, message_rx, shutdown_token);
        });
        
        // Return handle with modified channel routing
        handle
    }
}
```

### Message Routing

Dedicated thread agents will:
1. Receive messages through a standard Acton `AgentHandle`
2. Route messages through a thread-safe channel to the dedicated thread
3. Process messages on the dedicated thread using the same handler system
4. Maintain full compatibility with broker subscriptions and reply addresses

### Shutdown Coordination

```rust
// Graceful shutdown sequence for dedicated threads
impl Drop for DedicatedThreadAgent {
    fn drop(&mut self) {
        self.shutdown_token.cancel();
        // Wait for thread completion with timeout
    }
}
```

## File Changes Required

### Core Files
1. `acton-core/src/common/types.rs` - Add `AgentThreading` enum
2. `acton-core/src/actor/agent_config.rs` - Extend `AgentConfig` with threading field
3. `acton-core/src/common/agent_runtime.rs` - Modify agent spawning logic
4. `acton-core/src/actor/managed_agent/started.rs` - Add dedicated thread event loop
5. `acton-core/src/actor/managed_agent.rs` - Add threading state tracking

### New Files
- `acton-core/src/actor/dedicated_thread.rs` - Dedicated thread management utilities

### Example Updates
- `acton-reactive/examples/fruit_market/main.rs` - Update Printer agent to use dedicated thread
- `acton-reactive/examples/dedicated_thread.rs` - New example showcasing the feature

## Configuration Options

### Agent-Level Configuration
```toml
[agent.printer]
threading = "dedicated"

[agent.calculator]
threading = "default"  # Optional, default behavior
```

### Runtime-Level Defaults
```toml
[runtime]
default_threading = "default"
max_dedicated_threads = 8
```

## Edge Cases Handled

1. **Thread Pool Exhaustion**: Configurable limits on dedicated threads
2. **Message Overflow**: Backpressure handling for thread communication channels
3. **Panic Recovery**: Dedicated thread panic handling with agent restart
4. **Cross-Thread Messaging**: Thread-safe message passing without data races
5. **Resource Cleanup**: Proper cleanup of thread-local resources on shutdown

## Performance Considerations

### When to Use Dedicated Threads
- **I/O-bound agents**: Terminal/console management, file I/O
- **Thread-local state**: Agents using thread-local storage
- **Blocking operations**: Agents that need to block synchronously
- **Resource isolation**: Agents managing exclusive resources

### When NOT to Use Dedicated Threads
- **CPU-intensive agents**: Use Tokio thread pool for better scheduling
- **High-frequency messaging**: Thread switching overhead
- **Memory-constrained environments**: Each dedicated thread uses more memory

### Memory Impact
- **Dedicated thread**: ~2MB stack + agent state per thread
- **Tokio task**: ~64KB stack + agent state per task
- **Recommendation**: Limit to <10 dedicated threads per application

## Testing Strategy

### Unit Tests
- [ ] Dedicated thread spawning and cleanup
- [ ] Message routing accuracy
- [ ] Shutdown coordination
- [ ] Error handling and recovery

### Integration Tests
- [ ] End-to-end dedicated thread agent lifecycle
- [ ] Message compatibility with regular agents
- [ ] Broker subscription functionality
- [ ] Supervision hierarchy support

### Performance Tests
- [ ] Throughput comparison: dedicated vs pooled threads
- [ ] Memory usage profiling
- [ ] Latency measurements for cross-thread messaging

### Example Tests
- [ ] Fruit market example with dedicated Printer thread
- [ ] Terminal state consistency verification
- [ ] Graceful shutdown behavior

## Migration Guide

### For Existing Agents
No changes required - existing agents will continue to use the default Tokio thread pool.

### For New Dedicated Thread Agents
```rust
// Before (using default thread pool)
let mut printer_builder = runtime.new_agent::<Printer>().await;

// After (using dedicated thread)
let mut printer_builder = runtime
    .new_agent_with_config::<Printer>(
        AgentConfig::new("printer")
            .with_threading(AgentThreading::Dedicated)
    ).await;
```

## Development Process

### Incremental Implementation
1. **Week 1**: Type system and configuration changes
2. **Week 2**: Dedicated thread spawning and lifecycle management
3. **Week 3**: Message routing and integration testing
4. **Week 4**: Example updates and performance optimization

### Development Rules
1. **One change at a time**: Small, focused commits
2. **Compile after each change**: Ensure `cargo check` passes
3. **Test incrementally**: Add tests with each feature addition
4. **Maintain backward compatibility**: No breaking changes to existing APIs
5. **Performance validation**: Benchmark key scenarios

## Future Enhancements

### Advanced Threading Models
- **Custom thread pools**: Allow agents to specify custom Tokio runtimes
- **Thread affinity**: Pin specific agents to CPU cores
- **Priority scheduling**: Thread priority configuration
- **Workload balancing**: Dynamic thread allocation based on load

### Monitoring & Observability
- **Thread metrics**: CPU usage, memory consumption per dedicated thread
- **Health monitoring**: Thread liveness and responsiveness
- **Performance profiling**: Detailed performance analytics
- **Resource limits**: Configurable resource constraints per thread

## Timeline

- **Week 1**: Core type system and configuration (Phase 1)
- **Week 2**: Runtime integration and threading (Phase 2)
- **Week 3**: Message dispatch and testing (Phase 3)
- **Week 4**: Example updates and documentation (Phases 4-5)
- **Week 5**: Performance optimization and release preparation

This feature will be implemented as Acton Reactive v6.0.0, maintaining full backward compatibility while adding powerful new capabilities for specialized agent use cases.