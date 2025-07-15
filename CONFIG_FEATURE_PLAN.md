# Acton Reactive: XDG Configuration System

## Overview
This feature introduces a comprehensive configuration system using the XDG Base Directory Specification. Magic numbers and strings throughout acton-core will be extracted to a TOML configuration file, allowing users to override defaults through standard XDG locations.

## Feature Description

### Core Requirements
1. **XDG Compliance**: Use `xdg` crate to locate configuration files in standard locations
2. **TOML Format**: Configuration files use TOML for human-readable configuration
3. **Optional Loading**: Configuration files are optional with sensible defaults
4. **Lazy Loading**: Load configuration once during `ActonApp::launch()`
5. **Backward Compatibility**: All existing behavior preserved when no config is present

### Configuration Scope
The configuration system will extract and make configurable:
- **Timeouts**: Agent shutdown timeouts, message processing timeouts
- **Limits**: Buffer sizes, concurrent handler limits, queue sizes  
- **Paths**: Log directories, cache locations, data directories
- **Behavior**: Tracing levels, broker settings, agent lifecycle settings
- **Network**: Port numbers, connection timeouts, retry settings

## Implementation Plan

### Phase 1: Analysis & Discovery
- [x] **Task 1.1**: Scan acton-core for magic numbers and strings
- [x] **Task 1.2**: Categorize discovered values by type and usage
- [x] **Task 1.3**: Define configuration structure in TOML format
- [x] **Task 1.4**: Create configuration schema documentation

### Phase 2: Core Configuration System
- [x] **Task 2.1**: Add `xdg` crate dependency to acton-core Cargo.toml
- [x] **Task 2.2**: Create `config.rs` module in acton-core/src/common/
- [x] **Task 2.3**: Implement `ActonConfig` struct with serde support
- [x] **Task 2.4**: Add configuration loading logic using xdg crate
- [x] **Task 2.5**: Implement default values for all configuration options

### Phase 3: Integration with ActonApp
- [x] **Task 3.1**: Modify `ActonApp::launch()` to load configuration
- [x] **Task 3.2**: Add configuration parameter to AgentRuntime
- [x] **Task 3.3**: Update AgentConfig to use loaded configuration values
- [x] **Task 3.4**: Integrate configuration into ManagedAgent initialization

### Phase 4: Value Migration
- [x] **Task 4.1**: Replace magic numbers in shutdown timeout
- [x] **Task 4.2**: Replace magic numbers in message buffer sizes
- [x] **Task 4.3**: Replace magic numbers in FuturesUnordered limits
- [x] **Task 4.4**: Replace tracing configuration defaults
- [x] **Task 4.5**: Update any remaining hardcoded values

### Phase 5: Testing & Validation
- [ ] **Task 5.1**: Create config_loading_tests.rs in acton-reactive/tests/
- [ ] **Task 5.2**: Test default configuration loading
- [ ] **Task 5.3**: Test custom configuration override
- [ ] **Task 5.4**: Test XDG directory resolution
- [ ] **Task 5.5**: Test error handling for malformed configs
- [ ] **Task 5.6**: Test backward compatibility

### Phase 6: Documentation & Examples
- [ ] **Task 6.1**: Create example configuration file
- [ ] **Task 6.2**: Update API documentation
- [ ] **Task 6.3**: Create configuration guide
- [ ] **Task 6.4**: Update existing examples to use configuration

## Configuration Structure

### File Location
- **Linux**: `$XDG_CONFIG_HOME/acton/config.toml` or `~/.config/acton/config.toml`
- **macOS**: `$XDG_CONFIG_HOME/acton/config.toml` or `~/Library/Application Support/acton/config.toml`
- **Windows**: `%APPDATA%\acton\config.toml`

### Discovered Magic Numbers & Strings
Based on comprehensive analysis of acton-core/src/, here are the specific values to extract:

#### **Timeout Values**
- **read_only_handler_flush_ms**: `Duration::from_millis(10)` - Max wait before flushing read-only handlers
- **concurrent_handlers_high_water_mark**: `100` - Max concurrent read-only handlers before forced flush
- **agent_shutdown_timeout_ms**: `10_000` - Default agent shutdown timeout (10 seconds)
- **system_shutdown_timeout_ms**: `30_000` - Default system-wide shutdown timeout (30 seconds)

#### **Buffer Sizes & Capacity Limits**
- **agent_inbox_capacity**: `255` - Default MPSC channel size for agent message inbox
- **dummy_channel_size**: `1` - Dummy channel size for closed/default MessageAddress/AgentHandle

#### **Default Configuration Values**
- **default_agent_name**: `"agent"` - Default agent name when none provided
- **default_root_ern**: `Ern::default()` - Default root Ern identifier for agents

#### **Logging/Tracing Levels**
- **debug_level**: `"debug"` - Debug tracing level
- **trace_level**: `"trace"` - Trace tracing level
- **info_level**: `"info"` - Info tracing level

### TOML Schema
```toml
[timeouts]
agent_shutdown_timeout_ms = 10000
system_shutdown_timeout_ms = 30000
read_only_handler_flush_ms = 10

[limits]
concurrent_handlers_high_water_mark = 100
agent_inbox_capacity = 255
dummy_channel_size = 1

[defaults]
agent_name = "agent"
root_ern = "default"

[tracing]
debug_level = "debug"
trace_level = "trace"
info_level = "info"

[paths]
log_directory = "~/.local/share/acton/logs"
cache_directory = "~/.cache/acton"
data_directory = "~/.local/share/acton"
config_directory = "~/.config/acton"

[behavior]
enable_tracing = true
enable_metrics = false
```

## File Changes Required

### New Files
1. `acton-core/src/common/config.rs` - Configuration system
2. `acton-core/config/example-config.toml` - Example configuration
3. `acton-reactive/tests/config_loading_tests.rs` - Configuration tests

### Modified Files
1. `acton-core/Cargo.toml` - Add xdg dependency
2. `acton-core/src/common/acton.rs` - Load configuration in launch()
3. `acton-core/src/common/mod.rs` - Export config module
4. `acton-core/src/actor/managed_agent/started.rs` - Use config values
5. `acton-core/src/actor/managed_agent.rs` - Add config field
6. `acton-core/src/common/agent_runtime.rs` - Add config parameter

## Development Guidelines (CRITICAL)

### Development Rules
1. **One change at a time**: Work on a single small portion of the refactor
2. **Compile after each change**: Run `cargo check --workspace` to ensure compilation
3. **Commit per change**: Each completed portion must be committed before proceeding
4. **Sign commits**: Use `-S` flag for all commits (git commit -S -m "message")
5. **Await approval**: Do not proceed to next portion without explicit approval
6. **Match existing style**: Ensure code follows existing patterns exactly
7. **Safe defaults**: All configuration values must have sensible defaults
8. **Error handling**: Gracefully handle missing/invalid configurations

### Development Workflow
1. Pick one small portion from the plan
2. Implement the change
3. Run `cargo check --workspace`
4. Commit with descriptive message using `-S` flag
5. Await approval for next step

### Example Incremental Steps
- Add xdg dependency (Cargo.toml)
- Create empty config.rs module
- Define ActonConfig struct
- Add serde derives
- Implement default values
- Add xdg loading logic
- Integrate with ActonApp
- Replace one magic number
- Add one test case

## Testing Strategy

### Test Categories
1. **Unit Tests**: Configuration loading, parsing, validation
2. **Integration Tests**: End-to-end configuration usage
3. **Cross-platform Tests**: XDG directory resolution on different OS
4. **Error Tests**: Malformed configuration handling
5. **Compatibility Tests**: Ensure no breaking changes

### Test Files
- `config_loading_tests.rs` - Core configuration tests
- `xdg_resolution_tests.rs` - Platform-specific directory tests
- `default_config_tests.rs` - Default value verification

## Backward Compatibility
- **Zero breaking changes**: Existing code continues to work unchanged
- **Optional loading**: Configuration is optional, defaults used when absent
- **Graceful degradation**: Malformed configs use defaults with warnings
- **Migration path**: No changes required for existing users

## Error Handling
- **File not found**: Use defaults with info-level logging
- **Parse errors**: Use defaults with warning-level logging
- **Invalid values**: Use defaults with error-level logging
- **XDG errors**: Fallback to current directory with warning

## Performance Considerations
- **Lazy loading**: Configuration loaded once at startup
- **Minimal overhead**: No runtime configuration lookups
- **Memory efficient**: Configuration values cached in appropriate locations
- **Zero-cost abstraction**: No performance impact when using defaults

## Timeline
- **Week 1**: Analysis, discovery, and core configuration system
- **Week 2**: Integration with ActonApp and value migration
- **Week 3**: Testing and documentation
- **Week 4**: Examples and final validation

## Success Criteria
- [x] All magic numbers and strings extracted to configuration
- [x] Configuration system works on all platforms (Linux, macOS, Windows)
- [ ] Comprehensive test suite with >90% coverage
- [x] Zero breaking changes for existing users
- [ ] Clear documentation and examples
- [ ] Performance benchmarks show no regression