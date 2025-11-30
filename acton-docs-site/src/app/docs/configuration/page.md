---
title: Configuration
nextjs:
  metadata:
    title: Configuration - acton-reactive
    description: All configuration options available in acton-reactive, including file locations, TOML format, and runtime customization.
---

This guide covers all configuration options available in `acton-reactive`, including file locations, TOML format, and runtime customization.

---

## Configuration File Locations

`acton-reactive` follows the XDG Base Directory Specification for configuration file locations.

### Search Order

The framework searches for `config.toml` in these locations (in order):

| Platform | Primary Location | Fallback |
|----------|-----------------|----------|
| Linux | `$XDG_CONFIG_HOME/acton/config.toml` | `~/.config/acton/config.toml` |
| macOS | `$XDG_CONFIG_HOME/acton/config.toml` | `~/Library/Application Support/acton/config.toml` |
| Windows | `%APPDATA%/acton/config.toml` | - |

### Behavior

- If no configuration file is found, default values are used
- If a configuration file exists but is malformed, an error is logged and defaults are used
- Configuration is loaded once at startup and cached globally

---

## Configuration Sections

### Timeouts

Control various timeout behaviors (all values in **milliseconds**).

```toml
[timeouts]
# Timeout for individual agent shutdown
agent_shutdown = 10000      # 10 seconds

# Timeout for entire system shutdown
system_shutdown = 30000     # 30 seconds

# Maximum wait before flushing concurrent read-only handlers
read_only_handler_flush = 10  # 10 milliseconds
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `agent_shutdown` | `u64` | `10000` | Maximum time to wait for a single agent to stop gracefully |
| `system_shutdown` | `u64` | `30000` | Maximum time to wait for the entire system to shutdown |
| `read_only_handler_flush` | `u64` | `10` | Timeout before forcing a flush of pending read-only handlers |

---

### Limits

Control capacity and resource limits.

```toml
[limits]
# Maximum concurrent read-only handlers before forced flush
concurrent_handlers_high_water_mark = 100

# MPSC channel buffer size for agent message inboxes
agent_inbox_capacity = 255

# Size for dummy/placeholder channels
dummy_channel_size = 1
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `concurrent_handlers_high_water_mark` | `usize` | `100` | Maximum number of concurrent `act_on` handlers before they're flushed |
| `agent_inbox_capacity` | `usize` | `255` | Buffer size for agent message queues (backpressure threshold) |
| `dummy_channel_size` | `usize` | `1` | Size for internal placeholder channels |

#### Understanding Handler Limits

Read-only handlers (`act_on`) can execute concurrently. The `concurrent_handlers_high_water_mark` prevents unbounded concurrency:

```text
Messages arrive: M1, M2, M3, ... M100
├── Handler for M1 spawned
├── Handler for M2 spawned
├── ...
├── Handler for M100 spawned
└── HWM reached: Wait for all 100 to complete before processing more
```

---

### Defaults

Default values used when creating agents.

```toml
[defaults]
# Default agent name when none provided
agent_name = "agent"

# Default root ERN identifier
root_ern = "default"
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `agent_name` | `String` | `"agent"` | Name assigned when `new_agent()` is called without a name |
| `root_ern` | `String` | `"default"` | Base identifier for the root namespace |

---

### Tracing

Configure logging and tracing levels.

```toml
[tracing]
# Verbosity settings (used by tracing-subscriber)
debug = "debug"
trace = "trace"
info = "info"
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `debug` | `String` | `"debug"` | Debug level filter string |
| `trace` | `String` | `"trace"` | Trace level filter string |
| `info` | `String` | `"info"` | Info level filter string |

---

### Paths

Directory paths for various file storage needs.

```toml
[paths]
# Log file directory
logs = "~/.local/share/acton/logs"

# Cache directory
cache = "~/.cache/acton"

# Data storage directory
data = "~/.local/share/acton"

# Configuration directory
config = "~/.config/acton"
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `logs` | `String` | `~/.local/share/acton/logs` | Directory for log files |
| `cache` | `String` | `~/.cache/acton` | Directory for cached data |
| `data` | `String` | `~/.local/share/acton` | Directory for persistent data |
| `config` | `String` | `~/.config/acton` | Directory for configuration |

---

### Behavior

Toggle behavioral features on/off.

```toml
[behavior]
# Enable structured tracing output
enable_tracing = true

# Enable metrics collection
enable_metrics = false
```

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable_tracing` | `bool` | `true` | Enable structured logging via `tracing` |
| `enable_metrics` | `bool` | `false` | Enable metrics collection (when implemented) |

---

## Complete Reference

### All Configuration Options

```toml
# acton-reactive Configuration
# Place this file at: ~/.config/acton/config.toml

[timeouts]
agent_shutdown = 10000           # ms - Individual agent shutdown timeout
system_shutdown = 30000          # ms - System-wide shutdown timeout
read_only_handler_flush = 10     # ms - Read-only handler flush timeout

[limits]
concurrent_handlers_high_water_mark = 100  # Max concurrent act_on handlers
agent_inbox_capacity = 255                  # Agent message queue size
dummy_channel_size = 1                      # Placeholder channel size

[defaults]
agent_name = "agent"             # Default agent name
root_ern = "default"             # Default root ERN

[tracing]
debug = "debug"
trace = "trace"
info = "info"

[paths]
logs = "~/.local/share/acton/logs"
cache = "~/.cache/acton"
data = "~/.local/share/acton"
config = "~/.config/acton"

[behavior]
enable_tracing = true
enable_metrics = false
```

---

## Example Configurations

### Development Configuration

Optimized for development with more verbose logging and shorter timeouts:

```toml
# ~/.config/acton/config.toml (Development)

[timeouts]
agent_shutdown = 5000            # 5 seconds - fail fast
system_shutdown = 10000          # 10 seconds
read_only_handler_flush = 5      # Faster flush

[limits]
concurrent_handlers_high_water_mark = 50   # Lower for debugging
agent_inbox_capacity = 100                  # Smaller queues
dummy_channel_size = 1

[tracing]
debug = "debug"
trace = "trace"
info = "info"

[behavior]
enable_tracing = true
enable_metrics = true            # Enable for development insights
```

### Production Configuration

Optimized for production with higher capacity and longer timeouts:

```toml
# /etc/acton/config.toml (Production)

[timeouts]
agent_shutdown = 30000           # 30 seconds - graceful shutdown
system_shutdown = 60000          # 60 seconds
read_only_handler_flush = 50     # More batching

[limits]
concurrent_handlers_high_water_mark = 500  # Higher throughput
agent_inbox_capacity = 1000                 # Larger buffers
dummy_channel_size = 1

[tracing]
debug = "warn"                   # Less verbose
trace = "error"
info = "info"

[paths]
logs = "/var/log/acton"
cache = "/var/cache/acton"
data = "/var/lib/acton"
config = "/etc/acton"

[behavior]
enable_tracing = true
enable_metrics = true
```

---

## Programmatic Access

### Accessing Configuration

Configuration is available via the global `CONFIG` static:

```rust
use acton_reactive::common::config::CONFIG;

fn example() {
    // Access timeout settings
    let shutdown_timeout = CONFIG.timeouts.system_shutdown;

    // Access limits
    let inbox_size = CONFIG.limits.agent_inbox_capacity;

    // Access as Duration
    let duration = CONFIG.system_shutdown_timeout();
}
```

### Configuration Loading

The configuration is loaded lazily on first access:

```rust
use acton_reactive::common::config::ActonConfig;

fn custom_load() {
    // Load manually (usually not needed)
    let config = ActonConfig::load();

    // Or use the global instance
    use acton_reactive::common::config::CONFIG;
    let _ = &*CONFIG; // Force load
}
```

---

## IPC Configuration

When the `ipc` feature is enabled, additional configuration options are available.

### IpcConfig Structure

```rust
pub struct IpcConfig {
    /// Unix socket path for IPC listener
    pub socket_path: PathBuf,

    /// Maximum concurrent connections
    pub max_connections: usize,

    /// Connection timeout
    pub connection_timeout: Duration,

    /// Rate limiting configuration
    pub rate_limit: Option<RateLimitConfig>,
}
```

### Default IPC Values

| Option | Default | Description |
|--------|---------|-------------|
| `socket_path` | `/tmp/acton.sock` | Unix socket file path |
| `max_connections` | `100` | Maximum simultaneous connections |
| `connection_timeout` | `30s` | Idle connection timeout |
| `rate_limit` | `None` | Optional rate limiting |

### Configuring IPC Programmatically

```rust
use acton_reactive::prelude::*;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(feature = "ipc")]
async fn setup_ipc(runtime: &mut AgentRuntime) {
    use acton_reactive::common::ipc::IpcConfig;

    let ipc_config = IpcConfig {
        socket_path: PathBuf::from("/var/run/myapp/acton.sock"),
        max_connections: 50,
        connection_timeout: Duration::from_secs(60),
        rate_limit: None,
    };

    // Configure IPC with custom settings
    runtime.set_ipc_config(ipc_config);

    // Start the listener
    let listener = runtime.start_ipc_listener().await.expect("Failed to start IPC");
}
```

---

## Best Practices

### 1. Use Sensible Defaults

The default configuration works well for most use cases. Only override values when you have specific requirements.

### 2. Adjust Inbox Capacity Based on Load

```toml
# High-throughput scenarios
[limits]
agent_inbox_capacity = 1000

# Memory-constrained scenarios
[limits]
agent_inbox_capacity = 50
```

### 3. Set Appropriate Shutdown Timeouts

Consider your application's cleanup requirements:

```toml
[timeouts]
# Simple apps: shorter timeouts
agent_shutdown = 5000

# Complex apps with DB connections, file I/O: longer timeouts
agent_shutdown = 30000
```

### 4. Monitor High Water Mark

If you see frequent handler flushes in logs, consider increasing the limit:

```toml
[limits]
concurrent_handlers_high_water_mark = 200
```

### 5. Use Different Configs Per Environment

```shell
# Development
export XDG_CONFIG_HOME=./config/dev
cargo run

# Production
export XDG_CONFIG_HOME=/etc/myapp
./my-acton-app
```
