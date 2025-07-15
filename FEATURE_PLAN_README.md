# Acton Reactive v5.0.0: Concurrent Read-Only Message Handlers

## Overview
This feature introduces concurrent read-only message handlers to the Acton Reactive framework while maintaining backward compatibility with existing sequential mutable handlers through a clear API redesign.

## Breaking Changes (Major Version Bump)
- **Method renaming**: `act_on::<T>()` → `mutate_on::<T>()` for mutable handlers
- **New methods**: `act_on::<T>()` now provides concurrent read-only handlers
- **Version**: v5.0.0 due to breaking API changes

## Feature Description

### New Handler Types
1. **Read-Only Concurrent Handlers** (`act_on::<T>`)
   - Operate on `&ManagedAgent` (immutable reference)
   - Executed concurrently using `FuturesUnordered`
   - No message ordering guarantees
   - Ideal for queries, logging, monitoring

2. **Mutable Sequential Handlers** (`mutate_on::<T>`)
   - Operate on `&mut ManagedAgent` (mutable reference)
   - Executed sequentially to maintain state consistency
   - Preserves message ordering
   - Ideal for state mutations

### API Changes

#### Before (v4.x)
```rust
builder.act_on::<Increment>(|agent, _| {
    agent.model.count += 1;  // mutation
    AgentReply::immediate()
});
```

#### After (v5.0)
```rust
// Read-only concurrent handler
builder.act_on::<QueryState>(|agent, _| {
    let state = &agent.model;
    // read-only access, concurrent execution
    AgentReply::immediate()
});

// Mutable sequential handler  
builder.mutate_on::<Increment>(|agent, _| {
    agent.model.count += 1;  // mutation
    AgentReply::immediate()
});
```

## Implementation Plan

### Phase 1: Type System Extensions
- [x] Extend `ReactorItem` enum in `src/common/types.rs`
- [x] Add new handler types for read-only operations
- [x] Create separate storage maps for read-only vs mutable handlers

### Phase 2: Registration API
- [ ] Rename existing `act_on` methods to `mutate_on`
- [ ] Create new `act_on` methods for read-only handlers
- [ ] Update `idle.rs` with new registration methods

### Phase 3: Message Dispatch
- [ ] Modify `wake()` function in `started.rs`
- [ ] Add `FuturesUnordered` buffer for read-only handlers
- [ ] Implement concurrent execution logic
- [ ] Add buffer management and flushing

### Phase 4: Testing & Examples
- [ ] Update all existing tests to use new API
- [ ] Add comprehensive tests for concurrent handlers
- [ ] Update all examples with new method names
- [ ] Add new examples showcasing concurrent patterns

### Phase 5: Documentation
- [ ] Update API documentation
- [ ] Create migration guide
- [ ] Update README and guides

## File Changes Required

### Core Files
1. `src/common/types.rs` - New types and enums
2. `src/actor/managed_agent/idle.rs` - Registration methods
3. `src/actor/managed_agent/started.rs` - Dispatch logic
4. `src/actor/managed_agent.rs` - Add buffer field

### Test Files (to be updated)
- `acton-reactive/tests/actor_tests.rs`
- `acton-reactive/tests/messaging_tests.rs`
- `acton-reactive/tests/result_error_handler_tests.rs`
- All files in `tests/setup/`

### Examples (to be updated)
- `acton-reactive/examples/basic.rs`
- `acton-reactive/examples/broadcast.rs`
- `acton-reactive/examples/fruit_market.rs`
- `acton-reactive/examples/lifecycles.rs`

## Configuration Options
- **Buffer size**: Configurable limit for concurrent read-only handlers
- **Flush interval**: Time-based or count-based buffer flushing
- **Backpressure**: Mechanism to prevent buffer overflow

## Migration Guide
1. **Find and replace**: `act_on::<` → `mutate_on::<`
2. **Review handlers**: Ensure mutable operations use `mutate_on`
3. **Add new handlers**: Use `act_on` for read-only operations
4. **Test thoroughly**: Verify all behavior is preserved

## Edge Cases Handled
- Buffer overflow protection
- Graceful shutdown with pending futures
- Error handling in concurrent contexts
- Message ordering preservation for mutable handlers
- Thread safety for read-only operations

## Performance Considerations
- Concurrent read-only handlers improve throughput for non-mutating operations
- Mutable handlers remain sequential to prevent race conditions
- Configurable buffer size for memory/performance trade-offs
- Zero-cost abstraction for existing sequential patterns

## Testing Strategy
1. **Unit tests**: Handler registration and dispatch
2. **Integration tests**: Concurrent vs sequential behavior
3. **Stress tests**: Buffer management under load
4. **Migration tests**: Ensure v4.x patterns work with v5.0
5. **Examples**: Real-world usage patterns

## Development Process (CRITICAL)
**This feature must be developed incrementally, one small change at a time.**

### Development Rules
1. **One change at a time**: Work on a single small portion of the refactor
2. **Compile after each change**: Run `cargo check` to ensure compilation
3. **Commit per change**: Each completed portion must be committed before proceeding
4. **Await approval**: Do not proceed to next portion without explicit approval
5. **Match existing style**: Ensure code follows existing patterns exactly
6. **No Mutexes**: Use message passing exclusively for concurrency (existing pattern)
7. **Safe and idiomatic**: Ensure thread safety through actor model design
8. **No Claude attribution**: Commit messages should not include Claude attribution

### Development Workflow
1. Pick one small portion from the plan
2. Implement the change
3. Run `cargo check --workspace`
4. Commit with descriptive message
5. Await approval for next step

### Example Incremental Steps
- Add new type aliases (types.rs)
- Extend ReactorItem enum (types.rs)  
- Add new registration method signatures (idle.rs)
- Update storage structures (managed_agent.rs)
- Modify dispatch logic (started.rs)
- Update tests one file at a time

## Timeline
- **Week 1**: Type system and API changes (incremental commits)
- **Week 2**: Message dispatch implementation (incremental commits)
- **Week 3**: Testing and example updates (incremental commits)
- **Week 4**: Documentation and release preparation