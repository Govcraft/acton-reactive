/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     you may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for the Acton Reactive framework
/// 
/// This struct contains all configurable values for the Acton framework,
/// loaded from TOML files in XDG-compliant directories.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ActonConfig {
    /// Timeout configuration
    pub timeouts: TimeoutConfig,
    /// Limits and capacity configuration
    pub limits: LimitsConfig,
    /// Default values configuration
    pub defaults: DefaultsConfig,
    /// Tracing and logging configuration
    pub tracing: TracingConfig,
    /// Path configuration for various directories
    pub paths: PathsConfig,
    /// Behavioral configuration switches
    pub behavior: BehaviorConfig,
}

/// Timeout-related configuration values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutConfig {
    /// Default agent shutdown timeout in milliseconds
    pub agent_shutdown_timeout_ms: u64,
    /// Default system-wide shutdown timeout in milliseconds
    pub system_shutdown_timeout_ms: u64,
    /// Maximum wait time before flushing read-only handler futures
    pub read_only_handler_flush_ms: u64,
}

/// Limits and capacity configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// Maximum concurrent read-only handlers before forced flush
    pub concurrent_handlers_high_water_mark: usize,
    /// Default MPSC channel size for agent message inbox
    pub agent_inbox_capacity: usize,
    /// Dummy channel size for closed/default channels
    pub dummy_channel_size: usize,
}

/// Default configuration values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefaultsConfig {
    /// Default agent name when none provided
    pub agent_name: String,
    /// Default root Ern identifier
    pub root_ern: String,
}

/// Tracing and logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracingConfig {
    /// Debug tracing level
    pub debug_level: String,
    /// Trace tracing level
    pub trace_level: String,
    /// Info tracing level
    pub info_level: String,
}

/// Path configuration for various directories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    /// Directory for log files
    pub log_directory: String,
    /// Directory for cache files
    pub cache_directory: String,
    /// Directory for data files
    pub data_directory: String,
    /// Directory for configuration files
    pub config_directory: String,
}

/// Behavioral configuration switches
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehaviorConfig {
    /// Enable tracing
    pub enable_tracing: bool,
    /// Enable metrics collection
    pub enable_metrics: bool,
}

impl Default for ActonConfig {
    fn default() -> Self {
        Self {
            timeouts: TimeoutConfig::default(),
            limits: LimitsConfig::default(),
            defaults: DefaultsConfig::default(),
            tracing: TracingConfig::default(),
            paths: PathsConfig::default(),
            behavior: BehaviorConfig::default(),
        }
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            agent_shutdown_timeout_ms: 10_000,
            system_shutdown_timeout_ms: 30_000,
            read_only_handler_flush_ms: 10,
        }
    }
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            concurrent_handlers_high_water_mark: 100,
            agent_inbox_capacity: 255,
            dummy_channel_size: 1,
        }
    }
}

impl Default for DefaultsConfig {
    fn default() -> Self {
        Self {
            agent_name: "agent".to_string(),
            root_ern: "default".to_string(),
        }
    }
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            debug_level: "debug".to_string(),
            trace_level: "trace".to_string(),
            info_level: "info".to_string(),
        }
    }
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            log_directory: "~/.local/share/acton/logs".to_string(),
            cache_directory: "~/.cache/acton".to_string(),
            data_directory: "~/.local/share/acton".to_string(),
            config_directory: "~/.config/acton".to_string(),
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            enable_tracing: true,
            enable_metrics: false,
        }
    }
}

impl ActonConfig {
    /// Convert timeout values to Duration
    pub fn agent_shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.timeouts.agent_shutdown_timeout_ms)
    }

    pub fn system_shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.timeouts.system_shutdown_timeout_ms)
    }

    pub fn read_only_handler_flush(&self) -> Duration {
        Duration::from_millis(self.timeouts.read_only_handler_flush_ms)
    }

    /// Load configuration from XDG-compliant locations
    /// 
    /// This function attempts to load configuration from the following locations
    /// in order of preference:
    /// 1. `$XDG_CONFIG_HOME/acton/config.toml` (Linux/macOS)
    /// 2. `~/.config/acton/config.toml` (Linux fallback)
    /// 3. `~/Library/Application Support/acton/config.toml` (macOS fallback)
    /// 4. `%APPDATA%/acton/config.toml` (Windows)
    /// 
    /// If no configuration file is found, returns the default configuration.
    /// If a configuration file exists but is malformed, logs an error and uses defaults.
    pub fn load() -> Self {
        use tracing::{error, info};
        
        // Get the XDG base directories
        let xdg_dirs = match xdg::BaseDirectories::with_prefix("acton") {
            Ok(dirs) => dirs,
            Err(e) => {
                error!("Failed to initialize XDG directories: {}", e);
                return Self::default();
            }
        };

        // Try to find the configuration file
        let config_path = xdg_dirs.find_config_file("config.toml");

        match config_path {
            Some(path) => {
                info!("Loading configuration from: {}", path.display());
                match std::fs::read_to_string(&path) {
                    Ok(config_str) => {
                        match toml::from_str::<ActonConfig>(&config_str) {
                            Ok(config) => {
                                info!("Successfully loaded configuration");
                                config
                            }
                            Err(e) => {
                                error!("Failed to parse configuration file {}: {}", path.display(), e);
                                Self::default()
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to read configuration file {}: {}", path.display(), e);
                        Self::default()
                    }
                }
            }
            None => {
                info!("No configuration file found, using defaults");
                Self::default()
            }
        }
    }

    /// Get the actual configuration directory path
    /// 
    /// Returns the directory where configuration files should be stored
    /// according to XDG Base Directory Specification
    pub fn config_directory() -> std::path::PathBuf {
        match xdg::BaseDirectories::with_prefix("acton") {
            Ok(dirs) => dirs.get_config_home(),
            Err(_) => std::path::PathBuf::from("~/.config/acton"),
        }
    }
}