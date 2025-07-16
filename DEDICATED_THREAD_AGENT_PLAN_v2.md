# Acton Reactive v6.0.0: Dedicated Thread Agents (Revised)

## Overview (Revised)

After deeper analysis, we can achieve dedicated-thread behavior using Tokio's existing `spawn_blocking` mechanism rather than spawning raw OS threads. This approach is more idiomatic to Tokio and provides better integration.

## Tokio's Built-in Solutions

### 1. `spawn_blocking` - The Right Tool
```rust
// Tokio's spawn_blocking runs the task on a dedicated blocking thread
let handle = tokio::task::spawn_blocking(|| {
    // This runs on a dedicated thread from the blocking thread pool
    // Perfect for I/O-bound agents like terminal management
});
```

**Key benefits:**
- Dedicated OS thread per task (from blocking pool)
- Automatic thread management by Tokio
- Proper integration with Tokio's runtime
- Configurable thread pool limits
- Graceful shutdown handling

### 2. `spawn_blocking` vs Raw Threads

| Aspect | `spawn_blocking` | Raw OS Thread |
|--------|------------------|---------------|
| Thread Management | Tokio handles it | Manual |
| Pool Limits | Configurable | Manual |
| Runtime Integration | Native | Requires bridging |
| Shutdown Coordination | Automatic | Manual |
| Resource Cleanup | Automatic | Manual |

## Revised Implementation Plan

### Phase 1: Configuration Addition (Simplified)
- [ ] Add `AgentThreading` enum: `Default` | `Blocking`
- [ ] Extend `AgentConfig` with threading option
- [ ] No new files needed

### Phase 2: Runtime Integration (Minimal)
- [ ] Modify `AgentRuntime::spawn_agent` to use `spawn_blocking` for `Blocking` agents
- [ ] Update `ManagedAgent::wake()` to detect blocking thread mode
- [ ] Use existing message channels (no thread bridging needed)

### Implementation Details

```rust
// Simplified dedicated thread spawning
impl AgentRuntime {
    async fn spawn_agent_blocking<M: Send + 'static>(
        &self,
        mut agent: ManagedAgent<Started, M>,
    ) -> AgentHandle {
        let handle = agent.handle().clone();
        
        // Use spawn_blocking instead of raw threads
        let task = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            
            // Run agent on dedicated blocking thread
            rt.block_on(async {
                agent.wake().await;
            });
        });
        
        // Store task handle for shutdown coordination
        handle.tracker().spawn(task);
        handle
    }
}
```

### Configuration Usage

```rust
// Simple configuration
let mut printer_builder = runtime
    .new_agent_with_config::<Printer>(
        AgentConfig::new("printer")
            .with_threading(AgentThreading::Blocking)  // Uses spawn_blocking
    ).await;
```

## Advantages of This Approach

1. **Idiomatic Tokio**: Uses Tokio's intended API for blocking tasks
2. **Simpler Implementation**: No thread management code needed
3. **Better Integration**: Automatic shutdown, error handling, etc.
4. **Resource Efficient**: Tokio manages thread pool limits
5. **Backward Compatible**: Existing agents unaffected

## Configuration Options

### Runtime Configuration
```toml
[runtime.blocking_thread_pool]
max_threads = 512
thread_keep_alive = "10s"
thread_stack_size = "2MB"
```

## Testing Strategy (Simplified)

1. **Unit Tests**: Verify `spawn_blocking` selection based on configuration
2. **Integration**: Test Printer agent runs on blocking thread
3. **Performance**: Compare blocking vs default thread performance
4. **Compatibility**: Ensure message system works identically

## File Changes (Minimal)

- `acton-core/src/common/types.rs` - Add `AgentThreading` enum
- `acton-core/src/actor/agent_config.rs` - Add threading field to `AgentConfig`
- `acton-core/src/common/agent_runtime.rs` - Add spawn_blocking branch
- `acton-core/src/actor/managed_agent/started.rs` - Handle blocking thread context

## Migration Impact

**Zero breaking changes**. Existing code continues to work exactly as before. Only new configuration option added.

## Timeline (Accelerated)

- **Day 1**: Type system changes (2-3 hours)
- **Day 2**: Runtime integration (4-5 hours)
- **Day 3**: Testing and examples (3-4 hours)
- **Day 4**: Documentation and release

This revised approach leverages Tokio's existing capabilities rather than reinventing thread management, resulting in a much simpler and more robust implementation.