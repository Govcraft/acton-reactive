/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */

//! IPC configuration with XDG-compliant socket path handling.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

/// Configuration for IPC (Inter-Process Communication).
///
/// This struct contains all configurable values for the IPC subsystem,
/// including socket location, connection limits, and timeouts.
///
/// # XDG Compliance
///
/// Socket files are stored in `$XDG_RUNTIME_DIR/acton/<app_name>/` following
/// the XDG Base Directory Specification. Configuration files are loaded from
/// `$XDG_CONFIG_HOME/acton/<app_name>/ipc.toml`.
///
/// # Example Configuration File
///
/// ```toml
/// [socket]
/// # Override default socket path (optional)
/// # path = "/run/user/1000/acton/my_app/ipc.sock"
/// mode = 0o660
///
/// [limits]
/// max_connections = 100
/// max_message_size = 1048576  # 1 MiB
///
/// [timeouts]
/// request_timeout_ms = 30000
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IpcConfig {
    /// Socket configuration.
    pub socket: SocketConfig,
    /// Connection and message limits.
    pub limits: IpcLimitsConfig,
    /// Timeout configuration.
    pub timeouts: IpcTimeoutsConfig,
}

/// Socket-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SocketConfig {
    /// Override the default socket path.
    ///
    /// If `None`, the socket is created at
    /// `$XDG_RUNTIME_DIR/acton/<app_name>/ipc.sock`.
    pub path: Option<PathBuf>,

    /// Socket file permissions (Unix only).
    ///
    /// Default is `0o660` (owner and group read/write).
    #[cfg(unix)]
    pub mode: u32,

    /// Application name for sharding socket paths.
    ///
    /// If `None`, defaults to the binary name.
    pub app_name: Option<String>,
}

/// Limits for IPC operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IpcLimitsConfig {
    /// Maximum concurrent connections.
    pub max_connections: usize,

    /// Maximum message size in bytes.
    pub max_message_size: usize,
}

/// Timeout configuration for IPC operations.
///
/// All values are in milliseconds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IpcTimeoutsConfig {
    /// Request timeout in milliseconds.
    #[serde(rename = "request_timeout_ms")]
    pub request: u64,

    /// Connection read timeout in milliseconds.
    #[serde(rename = "read_timeout_ms")]
    pub read: u64,

    /// Connection write timeout in milliseconds.
    #[serde(rename = "write_timeout_ms")]
    pub write: u64,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            path: None,
            #[cfg(unix)]
            mode: 0o660,
            app_name: None,
        }
    }
}

impl Default for IpcLimitsConfig {
    fn default() -> Self {
        Self {
            max_connections: 100,
            max_message_size: 1_048_576, // 1 MiB
        }
    }
}

impl Default for IpcTimeoutsConfig {
    fn default() -> Self {
        Self {
            request: 30_000,
            read: 60_000,
            write: 30_000,
        }
    }
}

impl IpcConfig {
    /// Load IPC configuration from XDG-compliant locations.
    ///
    /// Attempts to load configuration from:
    /// 1. `$XDG_CONFIG_HOME/acton/ipc.toml`
    /// 2. Falls back to `~/.config/acton/ipc.toml`
    ///
    /// If no configuration file is found, returns the default configuration.
    #[must_use]
    pub fn load() -> Self {
        let xdg_dirs = match xdg::BaseDirectories::with_prefix("acton") {
            Ok(dirs) => dirs,
            Err(e) => {
                warn!("Failed to initialize XDG directories for IPC config: {}", e);
                return Self::default();
            }
        };

        xdg_dirs.find_config_file("ipc.toml").map_or_else(
            || {
                info!("No IPC configuration file found, using defaults");
                Self::default()
            },
            |path| {
                info!("Loading IPC configuration from: {}", path.display());
                match std::fs::read_to_string(&path) {
                    Ok(config_str) => match toml::from_str::<Self>(&config_str) {
                        Ok(config) => {
                            info!("Successfully loaded IPC configuration");
                            config
                        }
                        Err(e) => {
                            warn!(
                                "Failed to parse IPC configuration file {}: {}",
                                path.display(),
                                e
                            );
                            Self::default()
                        }
                    },
                    Err(e) => {
                        warn!(
                            "Failed to read IPC configuration file {}: {}",
                            path.display(),
                            e
                        );
                        Self::default()
                    }
                }
            },
        )
    }

    /// Get the resolved application name.
    ///
    /// Returns the configured `app_name` if set, otherwise extracts the
    /// binary name from the current executable path.
    #[must_use]
    pub fn app_name(&self) -> String {
        self.socket
            .app_name
            .clone()
            .unwrap_or_else(Self::default_app_name)
    }

    /// Get the default application name from the binary.
    fn default_app_name() -> String {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "acton".to_string())
    }

    /// Get the socket path for the IPC listener.
    ///
    /// Returns the configured socket path if set, otherwise constructs
    /// the default XDG-compliant path.
    ///
    /// # Default Path
    ///
    /// - Linux: `$XDG_RUNTIME_DIR/acton/<app_name>/ipc.sock`
    /// - Fallback: `/tmp/acton/<app_name>/ipc.sock`
    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.socket.path.clone().unwrap_or_else(|| {
            let app_name = self.app_name();

            // Use XDG_RUNTIME_DIR for ephemeral socket files
            let runtime_dir = std::env::var("XDG_RUNTIME_DIR")
                .map_or_else(|_| PathBuf::from("/tmp"), PathBuf::from);

            runtime_dir.join("acton").join(&app_name).join("ipc.sock")
        })
    }

    /// Get the socket directory (parent of socket path).
    #[must_use]
    pub fn socket_dir(&self) -> PathBuf {
        self.socket_path()
            .parent()
            .map_or_else(|| PathBuf::from("/tmp/acton"), PathBuf::from)
    }

    /// Get the request timeout as a `Duration`.
    #[must_use]
    pub const fn request_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeouts.request)
    }

    /// Get the read timeout as a `Duration`.
    #[must_use]
    pub const fn read_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeouts.read)
    }

    /// Get the write timeout as a `Duration`.
    #[must_use]
    pub const fn write_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeouts.write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IpcConfig::default();
        assert_eq!(config.limits.max_connections, 100);
        assert_eq!(config.limits.max_message_size, 1_048_576);
        assert_eq!(config.timeouts.request, 30_000);
    }

    #[test]
    fn test_socket_path_default() {
        let config = IpcConfig::default();
        let path = config.socket_path();

        // Should contain "acton" in the path
        assert!(path.to_string_lossy().contains("acton"));
        // Should end with ipc.sock
        assert!(path.to_string_lossy().ends_with("ipc.sock"));
    }

    #[test]
    fn test_socket_path_override() {
        let mut config = IpcConfig::default();
        config.socket.path = Some(PathBuf::from("/custom/path/socket.sock"));

        assert_eq!(
            config.socket_path(),
            PathBuf::from("/custom/path/socket.sock")
        );
    }

    #[test]
    fn test_app_name_override() {
        let mut config = IpcConfig::default();
        config.socket.app_name = Some("my_custom_app".to_string());

        assert_eq!(config.app_name(), "my_custom_app");
    }

    #[test]
    fn test_timeout_duration() {
        let config = IpcConfig::default();
        assert_eq!(
            config.request_timeout(),
            std::time::Duration::from_millis(30_000)
        );
    }

    #[test]
    fn test_socket_dir() {
        let config = IpcConfig::default();
        let dir = config.socket_dir();
        let path = config.socket_path();

        assert_eq!(dir, path.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn test_socket_mode() {
        let config = IpcConfig::default();
        assert_eq!(config.socket.mode, 0o660);
    }

    #[test]
    fn test_config_serialization() {
        let config = IpcConfig::default();
        let toml_str = toml::to_string(&config).unwrap();

        // Verify the config can be round-tripped
        let parsed: IpcConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.limits.max_connections, config.limits.max_connections);
    }
}
