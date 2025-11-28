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
use lazy_static::lazy_static;

/// Configuration for the Acton Reactive framework
/// 
/// This struct contains all configurable values for the Acton framework,
/// loaded from TOML files in XDG-compliant directories.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
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
    /// Convert system shutdown timeout to Duration
    pub const fn system_shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.timeouts.system_shutdown_timeout_ms)
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

        if let Some(path) = config_path {
            info!("Loading configuration from: {}", path.display());
            match std::fs::read_to_string(&path) {
                Ok(config_str) => {
                    match toml::from_str::<Self>(&config_str) {
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
        } else {
            info!("No configuration file found, using defaults");
            Self::default()
        }
    }
}

lazy_static! {
    /// Global configuration instance loaded from XDG-compliant locations
    pub static ref CONFIG: ActonConfig = ActonConfig::load();
}