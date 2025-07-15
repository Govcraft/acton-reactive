# Acton Reactive Configuration Guide

This guide explains how to configure Acton Reactive using the XDG Base Directory Specification.

## Overview

Acton Reactive uses a hierarchical configuration system that allows you to customize framework behavior without modifying your code. Configuration is optional - sensible defaults are provided when no configuration file is found.

## Configuration File Location

The configuration file is searched for in the following locations, in order:

- **Linux**: `$XDG_CONFIG_HOME/acton/config.toml` or `~/.config/acton/config.toml`
- **macOS**: `$XDG_CONFIG_HOME/acton/config.toml` or `~/Library/Application Support/acton/config.toml`
- **Windows**: `%APPDATA%\acton\config.toml`

## Quick Start

1. Create the configuration directory:
   ```bash
   mkdir -p ~/.config/acton
   ```

2. Copy the example configuration:
   ```bash
   cp examples/config.toml ~/.config/acton/config.toml
   ```

3. Edit the configuration file to match your needs.

## Configuration Sections

### Timeouts

Control various timeout values throughout the framework:

```toml
[timeouts]
agent_shutdown_timeout_ms = 10000    # 10 seconds for agent shutdown
system_shutdown_timeout_ms = 30000   # 30 seconds for system shutdown
read_only_handler_flush_ms = 10      # 10ms max wait for handler flush
```

### Limits

Configure capacity and performance limits:

```toml
[limits]
concurrent_handlers_high_water_mark = 100  # Max concurrent read-only handlers
agent_inbox_capacity = 255                 # MPSC channel size for agent inbox
dummy_channel_size = 1                     # Dummy channel size for defaults
```

### Defaults

Set default values for agent creation:

```toml
[defaults]
default_agent_name = "agent"  # Default name for new agents
default_root_ern = "default"  # Default root Ern identifier
```

### Tracing

Configure tracing levels:

```toml
[tracing]
debug_level = "debug"  # Debug log level
trace_level = "trace"  # Trace log level
info_level = "info"    # Info log level
```

### Paths

Customize directory locations:

```toml
[paths]
log_directory = "~/.local/share/acton/logs"    # Log files location
cache_directory = "~/.cache/acton"             # Cache files location
data_directory = "~/.local/share/acton"        # Data files location
config_directory = "~/.config/acton"           # Configuration files location
```

### Behavior

Enable or disable framework features:

```toml
[behavior]
enable_tracing = true   # Enable tracing output
enable_metrics = false  # Enable metrics collection
```

## Environment Variables

You can override the configuration directory using environment variables:

- `XDG_CONFIG_HOME`: Override the base configuration directory
- `XDG_DATA_HOME`: Override the base data directory
- `XDG_CACHE_HOME`: Override the base cache directory

## Use Cases

### Production Configuration

For production systems, consider these settings:

```toml
[timeouts]
agent_shutdown_timeout_ms = 5000
system_shutdown_timeout_ms = 15000

[limits]
agent_inbox_capacity = 512
concurrent_handlers_high_water_mark = 50

[tracing]
debug_level = "info"
trace_level = "warn"
info_level = "info"

[behavior]
enable_tracing = true
enable_metrics = true
```

### Development Configuration

For development, you might want more verbose logging:

```toml
[timeouts]
read_only_handler_flush_ms = 5  # Faster feedback

[tracing]
debug_level = "debug"
trace_level = "trace"

[behavior]
enable_tracing = true
enable_metrics = true
```

### Testing Configuration

For testing, you might want minimal timeouts:

```toml
[timeouts]
agent_shutdown_timeout_ms = 1000
system_shutdown_timeout_ms = 3000

[limits]
agent_inbox_capacity = 16  # Small for testing
```

## Error Handling

The configuration system is designed to be resilient:

- **Missing file**: Uses sensible defaults
- **Malformed values**: Uses defaults with warnings
- **Invalid paths**: Falls back to system defaults
- **Type errors**: Uses default values with informative messages

## Migration Guide

### From Default Configuration

No changes needed - existing code continues to work unchanged.

### From Hardcoded Values

If you previously modified source code to change timeouts or limits, migrate those values to your configuration file.

### Backward Compatibility

- All existing APIs remain unchanged
- Default behavior is preserved
- No breaking changes to public interfaces

## Examples

### Basic Usage

```rust
use acton_reactive::prelude::*;

#[acton_test]
async fn test_with_config() -> anyhow::Result<()> {
    let mut app = ActonApp::launch();  // Loads config automatically
    
    let agent_builder = app.new_agent::<MyAgent>().await;
    let handle = agent_builder.start().await;
    
    // Your existing code works unchanged
    Ok(())
}
```

### Custom Configuration Directory

```bash
# Set custom config directory
export XDG_CONFIG_HOME=/custom/config/path
# Then run your application
```

## Troubleshooting

### Configuration Not Loading

1. Check file permissions
2. Verify TOML syntax
3. Ensure file is in correct location
4. Check environment variables

### Performance Issues

1. Review timeout settings
2. Check capacity limits
3. Adjust tracing levels
4. Monitor memory usage

### Debugging Configuration

Enable debug logging to see configuration loading:

```bash
RUST_LOG=acton_core=debug cargo run
```

This will show:
- Configuration file location
- Values being loaded
- Any warnings or errors
- Fallback behavior

## Advanced Configuration

### Conditional Configuration

You can use conditional compilation for different environments:

```rust
#[cfg(debug_assertions)]
const CONFIG_PATH: &str = "dev-config.toml";

#[cfg(not(debug_assertions))]
const CONFIG_PATH: &str = "prod-config.toml";
```

### Configuration Validation

The configuration system validates values automatically:

- Timeouts must be positive integers
- Capacities must be positive integers
- Paths must be valid
- Log levels must be valid tracing levels

## Contributing

To add new configuration options:

1. Add field to `ActonConfig` struct in `acton-core/src/common/config.rs`
2. Provide default value in `impl Default for ActonConfig`
3. Update this documentation
4. Add example to `examples/config.toml`
5. Add test cases to ensure proper loading