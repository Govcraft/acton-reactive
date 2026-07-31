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
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// File name of the IPC configuration file in either searched location.
const CONFIG_FILE_NAME: &str = "ipc.toml";

/// Which of the two searched locations supplied the loaded configuration.
///
/// Reported in the load-time log so the effective file is unambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    /// `$XDG_CONFIG_HOME/acton/<app_name>/ipc.toml`.
    PerApp,
    /// `$XDG_CONFIG_HOME/acton/ipc.toml`, shared by every acton IPC server.
    Shared,
}

impl ConfigSource {
    /// Short, stable label for logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PerApp => "per-app",
            Self::Shared => "shared",
        }
    }
}

impl std::fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Choose between the per-application and shared configuration candidates.
///
/// The per-application file wins when both exist, so an application can override
/// machine-wide settings. Pure: it decides from the candidates it is handed and
/// touches no filesystem.
fn resolve_config_path(
    per_app: Option<PathBuf>,
    shared: Option<PathBuf>,
) -> Option<(PathBuf, ConfigSource)> {
    per_app
        .map(|path| (path, ConfigSource::PerApp))
        .or_else(|| shared.map(|path| (path, ConfigSource::Shared)))
}

/// Return `path` only when it names an existing file.
fn existing_file(path: PathBuf) -> Option<PathBuf> {
    path.is_file().then_some(path)
}

/// Configuration for IPC (Inter-Process Communication).
///
/// This struct contains all configurable values for the IPC subsystem,
/// including socket location, connection limits, rate limiting, and timeouts.
///
/// # XDG Compliance
///
/// Socket files are stored in `$XDG_RUNTIME_DIR/acton/<app_name>/` following
/// the XDG Base Directory Specification.
///
/// Configuration files are searched in two locations, in order, and the first
/// match wins (see [`IpcConfig::load`]):
///
/// 1. `$XDG_CONFIG_HOME/acton/<app_name>/ipc.toml` — per-application.
/// 2. `$XDG_CONFIG_HOME/acton/ipc.toml` — shared by every acton IPC server
///    on the machine.
///
/// The location actually used is logged at load time.
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
/// max_connections = 1024
/// max_message_size = 1048576  # 1 MiB
///
/// [rate_limit]
/// enabled = true
/// requests_per_second = 100
/// burst_size = 50
///
/// [timeouts]
/// request_timeout_ms = 30000
///
/// [shutdown]
/// drain_timeout_ms = 5000
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IpcConfig {
    /// Socket configuration.
    pub socket: SocketConfig,
    /// Connection and message limits.
    pub limits: IpcLimitsConfig,
    /// Rate limiting configuration.
    pub rate_limit: RateLimitConfig,
    /// Timeout configuration.
    pub timeouts: IpcTimeoutsConfig,
    /// Graceful shutdown configuration.
    pub shutdown: ShutdownConfig,
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
    ///
    /// Defaults to 1024. Connections beyond this limit are refused with
    /// [`IpcError::ConnectionLimitReached`](super::IpcError::ConnectionLimitReached),
    /// and the effective limit is logged when the listener starts.
    pub max_connections: usize,

    /// Maximum message size in bytes.
    pub max_message_size: usize,

    /// Buffer size for push notifications per connection.
    ///
    /// When a connection subscribes to message types, push notifications are
    /// buffered in a channel. If the buffer fills up because the client isn't
    /// reading fast enough, notifications will be dropped.
    pub push_buffer_size: usize,
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
    ///
    /// This applies to connections that do not have active subscriptions.
    /// Connections with subscriptions use `subscription_read` instead.
    #[serde(rename = "read_timeout_ms")]
    pub read: u64,

    /// Connection write timeout in milliseconds.
    #[serde(rename = "write_timeout_ms")]
    pub write: u64,

    /// Read timeout in milliseconds for connections with active subscriptions.
    ///
    /// Subscription clients often only receive data (push notifications) without
    /// sending messages. A value of 0 means no timeout for subscription connections,
    /// allowing them to remain connected indefinitely while receiving broadcasts.
    ///
    /// Default is 0 (no timeout for subscribers).
    #[serde(rename = "subscription_read_timeout_ms")]
    pub subscription_read: u64,
}

/// Rate limiting configuration for IPC connections.
///
/// Uses a token bucket algorithm: tokens are replenished at `requests_per_second`
/// rate up to `burst_size` maximum. Each request consumes one token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Whether rate limiting is enabled.
    pub enabled: bool,

    /// Maximum requests per second per connection.
    ///
    /// This is the sustained rate at which tokens are replenished.
    pub requests_per_second: u32,

    /// Maximum burst size (token bucket capacity).
    ///
    /// Allows short bursts of requests above the sustained rate.
    pub burst_size: u32,
}

/// Graceful shutdown configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ShutdownConfig {
    /// Maximum time in milliseconds to wait for in-flight requests to complete.
    #[serde(rename = "drain_timeout_ms")]
    pub drain_timeout: u64,
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
            // Sized from measured per-connection buffer reservation (~20 KiB:
            // push channel 100 x 112 B, writer channel 64 x ~128 B), giving a
            // ~20 MiB ceiling at the limit. The previous default of 100 capped
            // usable topologies at ~2 MiB worth of connections.
            max_connections: 1024,
            max_message_size: 1_048_576, // 1 MiB
            push_buffer_size: 100,       // 100 pending push notifications
        }
    }
}

impl Default for IpcTimeoutsConfig {
    fn default() -> Self {
        Self {
            request: 30_000,
            read: 60_000,
            write: 30_000,
            subscription_read: 0, // No timeout for subscribers by default
        }
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_second: 100,
            burst_size: 50,
        }
    }
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self {
            drain_timeout: 5_000, // 5 seconds
        }
    }
}

impl IpcConfig {
    /// Load IPC configuration from XDG-compliant locations.
    ///
    /// Two locations are searched, in order, and the first file that exists wins:
    ///
    /// 1. `$XDG_CONFIG_HOME/acton/<app_name>/ipc.toml` — per-application. `<app_name>`
    ///    is the current binary's file stem. (The `app_name` field inside the file
    ///    cannot be used here: it is only known once a file has been read.)
    /// 2. `$XDG_CONFIG_HOME/acton/ipc.toml` — shared by every acton IPC server on the
    ///    machine.
    ///
    /// The location used is logged at `INFO`, so it is clear which file took effect.
    /// If neither exists, the default configuration is returned.
    #[must_use]
    pub fn load() -> Self {
        let xdg_dirs = match xdg::BaseDirectories::with_prefix("acton") {
            Ok(dirs) => dirs,
            Err(e) => {
                warn!("Failed to initialize XDG directories for IPC config: {}", e);
                return Self::default();
            }
        };

        let app_name = Self::default_app_name();
        let per_app = xdg_dirs.find_config_file(PathBuf::from(&app_name).join(CONFIG_FILE_NAME));
        let shared = xdg_dirs.find_config_file(CONFIG_FILE_NAME);

        resolve_config_path(per_app, shared).map_or_else(
            || {
                info!(
                    app_name = %app_name,
                    "No IPC configuration file found in either the per-application or shared \
                     location, using defaults"
                );
                Self::default()
            },
            |(path, source)| Self::load_from_path(&path, source),
        )
    }

    /// Read and parse a configuration file, falling back to defaults on any failure.
    fn load_from_path(path: &Path, source: ConfigSource) -> Self {
        info!(
            source = source.as_str(),
            "Loading IPC configuration from: {}",
            path.display()
        );

        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(e) => {
                warn!(
                    "Failed to read IPC configuration file {}: {}",
                    path.display(),
                    e
                );
                return Self::default();
            }
        };

        match toml::from_str::<Self>(&contents) {
            Ok(config) => {
                info!(source = source.as_str(), "Successfully loaded IPC configuration");
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
        }
    }

    /// Load configuration from an explicit configuration root.
    ///
    /// Applies the same two-step search as [`IpcConfig::load`] against `config_root`
    /// instead of the XDG configuration directory:
    /// `config_root/<app_name>/ipc.toml`, then `config_root/ipc.toml`.
    ///
    /// This is the seam that makes the search order testable without mutating
    /// process-global environment variables.
    #[must_use]
    pub fn load_from_root(config_root: &Path, app_name: &str) -> Self {
        let per_app = existing_file(config_root.join(app_name).join(CONFIG_FILE_NAME));
        let shared = existing_file(config_root.join(CONFIG_FILE_NAME));

        resolve_config_path(per_app, shared).map_or_else(
            || {
                info!("No IPC configuration file found, using defaults");
                Self::default()
            },
            |(path, source)| Self::load_from_path(&path, source),
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

    /// Get the read timeout as an `Option<Duration>`.
    ///
    /// Returns `None` if the timeout is 0 (no timeout),
    /// otherwise returns `Some(Duration)`.
    #[must_use]
    pub const fn read_timeout(&self) -> Option<std::time::Duration> {
        if self.timeouts.read == 0 {
            None
        } else {
            Some(std::time::Duration::from_millis(self.timeouts.read))
        }
    }

    /// Get the write timeout as a `Duration`.
    #[must_use]
    pub const fn write_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeouts.write)
    }

    /// Get the subscription read timeout as an `Option<Duration>`.
    ///
    /// Returns `None` if the timeout is 0 (no timeout for subscribers),
    /// otherwise returns `Some(Duration)`.
    #[must_use]
    pub const fn subscription_read_timeout(&self) -> Option<std::time::Duration> {
        if self.timeouts.subscription_read == 0 {
            None
        } else {
            Some(std::time::Duration::from_millis(
                self.timeouts.subscription_read,
            ))
        }
    }

    /// Get the drain timeout as a `Duration`.
    ///
    /// This is the maximum time to wait for in-flight requests during shutdown.
    #[must_use]
    pub const fn drain_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.shutdown.drain_timeout)
    }

    /// Check if rate limiting is enabled.
    #[must_use]
    pub const fn is_rate_limited(&self) -> bool {
        self.rate_limit.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IpcConfig::default();
        assert_eq!(config.limits.max_connections, 1024);
        assert_eq!(config.limits.max_message_size, 1_048_576);
        assert_eq!(config.timeouts.request, 30_000);
    }

    #[test]
    fn per_app_candidate_wins_over_shared() {
        let resolved = resolve_config_path(
            Some(PathBuf::from("/cfg/my_app/ipc.toml")),
            Some(PathBuf::from("/cfg/ipc.toml")),
        );

        assert_eq!(
            resolved,
            Some((PathBuf::from("/cfg/my_app/ipc.toml"), ConfigSource::PerApp))
        );
    }

    #[test]
    fn shared_candidate_is_used_when_there_is_no_per_app_file() {
        let resolved = resolve_config_path(None, Some(PathBuf::from("/cfg/ipc.toml")));

        assert_eq!(
            resolved,
            Some((PathBuf::from("/cfg/ipc.toml"), ConfigSource::Shared))
        );
    }

    #[test]
    fn no_candidates_resolve_to_nothing() {
        assert_eq!(resolve_config_path(None, None), None);
    }

    #[test]
    fn config_source_labels_are_stable() {
        assert_eq!(ConfigSource::PerApp.as_str(), "per-app");
        assert_eq!(ConfigSource::Shared.as_str(), "shared");
        assert_eq!(ConfigSource::PerApp.to_string(), "per-app");
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

    /// Collects formatted log output so tests can assert on what was reported.
    #[derive(Clone, Default)]
    struct LogCapture {
        buffer: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    }

    impl LogCapture {
        fn contents(&self) -> String {
            let buffer = self.buffer.lock().expect("log buffer poisoned");
            String::from_utf8_lossy(&buffer).into_owned()
        }
    }

    impl std::io::Write for LogCapture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.buffer
                .lock()
                .expect("log buffer poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl tracing_subscriber::fmt::MakeWriter<'_> for LogCapture {
        type Writer = Self;

        fn make_writer(&self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `f` with log output captured.
    fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, String) {
        let capture = LogCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .finish();

        let value = tracing::subscriber::with_default(subscriber, f);
        let contents = capture.contents();
        (value, contents)
    }

    fn write_limits_config(path: &Path, max_connections: usize) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create config dir");
        }
        std::fs::write(
            path,
            format!("[limits]\nmax_connections = {max_connections}\n"),
        )
        .expect("write config");
    }

    /// The silent part is the bug: which file took effect must be in the log.
    #[test]
    fn loading_a_per_app_config_logs_that_it_was_the_per_app_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_limits_config(&dir.path().join("my_app").join(CONFIG_FILE_NAME), 7);

        let (config, logs) =
            capture_logs(|| IpcConfig::load_from_root(dir.path(), "my_app"));

        assert_eq!(config.limits.max_connections, 7);
        assert!(
            logs.contains("per-app"),
            "log should name the per-app source, got: {logs}"
        );
        assert!(
            logs.contains("my_app"),
            "log should name the file that was loaded, got: {logs}"
        );
    }

    #[test]
    fn loading_a_shared_config_logs_that_it_was_the_shared_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_limits_config(&dir.path().join(CONFIG_FILE_NAME), 11);

        let (config, logs) =
            capture_logs(|| IpcConfig::load_from_root(dir.path(), "my_app"));

        assert_eq!(config.limits.max_connections, 11);
        assert!(
            logs.contains("shared"),
            "log should name the shared source, got: {logs}"
        );
    }

    #[test]
    fn finding_no_config_file_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");

        let (config, logs) =
            capture_logs(|| IpcConfig::load_from_root(dir.path(), "my_app"));

        assert_eq!(
            config.limits.max_connections,
            IpcLimitsConfig::default().max_connections
        );
        assert!(
            logs.contains("No IPC configuration file found"),
            "log should say defaults were used, got: {logs}"
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
            std::time::Duration::from_secs(30)
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

    #[test]
    fn test_rate_limit_defaults() {
        let config = IpcConfig::default();
        assert!(config.is_rate_limited());
        assert_eq!(config.rate_limit.requests_per_second, 100);
        assert_eq!(config.rate_limit.burst_size, 50);
    }

    #[test]
    fn test_shutdown_defaults() {
        let config = IpcConfig::default();
        assert_eq!(config.shutdown.drain_timeout, 5_000);
        assert_eq!(
            config.drain_timeout(),
            std::time::Duration::from_secs(5)
        );
    }

    #[test]
    fn test_rate_limit_disabled() {
        let mut config = IpcConfig::default();
        config.rate_limit.enabled = false;
        assert!(!config.is_rate_limited());
    }
}
