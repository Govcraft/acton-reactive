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

//! Restart limiter for supervised actors with exponential backoff.
//!
//! This module provides Erlang/OTP-style restart limiting that prevents
//! restart loops by tracking restart attempts within a sliding time window
//! and applying exponential backoff between restarts.
//!
//! # Example
//!
//! ```rust,ignore
//! use acton_reactive::prelude::*;
//!
//! // Configure an actor with custom restart limits
//! let config = ActorConfig::new(Ern::with_root("worker")?, None, None)?
//!     .with_restart_limiter(RestartLimiterConfig {
//!         enabled: true,
//!         max_restarts: 3,
//!         window_secs: 30,
//!         initial_backoff_ms: 100,
//!         max_backoff_ms: 5000,
//!         backoff_multiplier: 2.0,
//!     });
//! ```

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Configuration for restart limiting and exponential backoff.
///
/// Controls how often an actor can be restarted within a time window
/// and the backoff delay between restart attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RestartLimiterConfig {
    /// Whether restart limiting is enabled.
    ///
    /// When disabled, restarts are allowed without limits.
    pub enabled: bool,

    /// Maximum restart attempts within the time window.
    ///
    /// When this limit is exceeded, the supervisor should escalate
    /// the failure to its parent.
    pub max_restarts: u32,

    /// Time window in seconds for counting restarts.
    ///
    /// Restart counts are reset after this period of no restarts.
    pub window_secs: u64,

    /// Initial backoff delay in milliseconds.
    ///
    /// This is the delay before the first restart attempt.
    pub initial_backoff_ms: u64,

    /// Maximum backoff delay in milliseconds.
    ///
    /// The backoff will be capped at this value regardless of
    /// how many consecutive restarts have occurred.
    pub max_backoff_ms: u64,

    /// Backoff multiplier for exponential growth.
    ///
    /// Each consecutive restart multiplies the backoff by this factor.
    /// For example, with multiplier 2.0 and initial 100ms:
    /// - First restart: 100ms
    /// - Second restart: 200ms
    /// - Third restart: 400ms
    pub backoff_multiplier: f64,
}

impl Default for RestartLimiterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_restarts: 5,
            window_secs: 60,
            initial_backoff_ms: 100,
            max_backoff_ms: 30_000,
            backoff_multiplier: 2.0,
        }
    }
}

impl RestartLimiterConfig {
    /// Create a disabled restart limiter configuration.
    ///
    /// Useful for testing or when restart limiting is not desired.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            max_restarts: 0,
            window_secs: 0,
            initial_backoff_ms: 0,
            max_backoff_ms: 0,
            backoff_multiplier: 0.0,
        }
    }

    /// Get the window duration.
    #[must_use]
    pub const fn window_duration(&self) -> Duration {
        Duration::from_secs(self.window_secs)
    }

    /// Get the initial backoff duration.
    #[must_use]
    pub const fn initial_backoff(&self) -> Duration {
        Duration::from_millis(self.initial_backoff_ms)
    }

    /// Get the maximum backoff duration.
    #[must_use]
    pub const fn max_backoff(&self) -> Duration {
        Duration::from_millis(self.max_backoff_ms)
    }
}

/// Tracks restart attempts and implements exponential backoff.
///
/// Uses a sliding window to count restarts within a configurable time period.
/// When the limit is exceeded, signals that the failure should be escalated
/// to the parent supervisor.
#[derive(Debug)]
pub struct RestartLimiter {
    /// Configuration for restart limits and backoff.
    config: RestartLimiterConfig,
    /// Timestamps of recent restart attempts (within the window).
    restart_timestamps: Vec<Instant>,
    /// Number of consecutive restarts (for backoff calculation).
    consecutive_restarts: usize,
}

impl RestartLimiter {
    /// Create a new restart limiter from configuration.
    #[must_use]
    pub const fn new(config: RestartLimiterConfig) -> Self {
        Self {
            config,
            restart_timestamps: Vec::new(),
            consecutive_restarts: 0,
        }
    }

    /// Check if a restart is allowed within the current window.
    ///
    /// This method prunes expired timestamps and checks if we're within
    /// the restart limit.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if restart is allowed
    /// - `Err(RestartLimitExceeded)` if the limit has been exceeded
    pub fn can_restart(&mut self) -> Result<(), RestartLimitExceeded> {
        if !self.config.enabled {
            return Ok(());
        }

        // Remove timestamps outside the window
        self.prune_expired();

        // Check if we're within the limit
        if self.restart_timestamps.len() >= self.config.max_restarts as usize {
            Err(RestartLimitExceeded {
                attempts: self.restart_timestamps.len(),
                max_restarts: self.config.max_restarts,
                window_secs: self.config.window_secs,
            })
        } else {
            Ok(())
        }
    }

    /// Record a restart attempt and return the backoff duration to wait.
    ///
    /// This should be called after `can_restart()` returns `Ok(())`.
    /// The caller should `tokio::time::sleep(backoff).await` before
    /// spawning the new actor.
    ///
    /// # Returns
    ///
    /// The duration to wait before restarting the actor.
    pub fn record_restart(&mut self) -> Duration {
        let now = Instant::now();
        self.restart_timestamps.push(now);

        // Calculate exponential backoff
        #[allow(clippy::cast_precision_loss)]
        let backoff_ms = self.config.initial_backoff_ms as f64
            * self.config.backoff_multiplier.powi(
                i32::try_from(self.consecutive_restarts).unwrap_or(i32::MAX),
            );
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss
        )]
        let capped_backoff_ms =
            (backoff_ms.min(self.config.max_backoff_ms as f64).max(0.0)) as u64;

        self.consecutive_restarts += 1;

        Duration::from_millis(capped_backoff_ms)
    }

    /// Reset the consecutive restart counter.
    ///
    /// Call this after a period of successful operation to reset the
    /// backoff calculation. The sliding window of restart timestamps
    /// is not affected.
    pub const fn reset_consecutive(&mut self) {
        self.consecutive_restarts = 0;
    }

    /// Get statistics for monitoring and logging.
    #[must_use]
    pub const fn stats(&self) -> RestartStats {
        RestartStats {
            restarts_in_window: self.restart_timestamps.len(),
            consecutive_restarts: self.consecutive_restarts,
            window_secs: self.config.window_secs,
            max_restarts: self.config.max_restarts,
        }
    }

    /// Get the number of restarts currently in the window.
    #[must_use]
    pub const fn restarts_in_window(&self) -> usize {
        self.restart_timestamps.len()
    }

    /// Get the number of consecutive restarts.
    #[must_use]
    pub const fn consecutive_restarts(&self) -> usize {
        self.consecutive_restarts
    }

    /// Calculate the backoff for the next restart without recording it.
    #[must_use]
    pub fn peek_backoff(&self) -> Duration {
        #[allow(clippy::cast_precision_loss)]
        let backoff_ms = self.config.initial_backoff_ms as f64
            * self.config.backoff_multiplier.powi(
                i32::try_from(self.consecutive_restarts).unwrap_or(i32::MAX),
            );
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss
        )]
        let capped_backoff_ms =
            (backoff_ms.min(self.config.max_backoff_ms as f64).max(0.0)) as u64;

        Duration::from_millis(capped_backoff_ms)
    }

    /// Remove timestamps that are outside the sliding window.
    fn prune_expired(&mut self) {
        let now = Instant::now();
        let window = Duration::from_secs(self.config.window_secs);
        self.restart_timestamps
            .retain(|&ts| now.duration_since(ts) < window);
    }
}

impl Clone for RestartLimiter {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            restart_timestamps: Vec::new(), // Don't clone timestamps
            consecutive_restarts: 0,        // Reset on clone
        }
    }
}

impl Default for RestartLimiter {
    fn default() -> Self {
        Self::new(RestartLimiterConfig::default())
    }
}

/// Error returned when the restart limit has been exceeded.
///
/// This indicates that the supervisor should escalate the failure
/// to its parent rather than attempting another restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartLimitExceeded {
    /// Number of restart attempts in the current window.
    pub attempts: usize,
    /// Maximum allowed restarts.
    pub max_restarts: u32,
    /// Window size in seconds.
    pub window_secs: u64,
}

impl std::fmt::Display for RestartLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "restart limit exceeded: {} attempts (max {}) in {} seconds",
            self.attempts, self.max_restarts, self.window_secs
        )
    }
}

impl std::error::Error for RestartLimitExceeded {}

/// Statistics about restart limiter state for monitoring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartStats {
    /// Number of restarts in the current window.
    pub restarts_in_window: usize,
    /// Number of consecutive restarts (affects backoff).
    pub consecutive_restarts: usize,
    /// Window size in seconds.
    pub window_secs: u64,
    /// Maximum allowed restarts.
    pub max_restarts: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let config = RestartLimiterConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_restarts, 5);
        assert_eq!(config.window_secs, 60);
        assert_eq!(config.initial_backoff_ms, 100);
        assert_eq!(config.max_backoff_ms, 30_000);
        assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn disabled_config_allows_all_restarts() {
        let config = RestartLimiterConfig::disabled();
        let mut limiter = RestartLimiter::new(config);

        // Should always succeed when disabled
        for _ in 0..100 {
            assert!(limiter.can_restart().is_ok());
            let _ = limiter.record_restart();
        }
    }

    #[test]
    fn limiter_allows_restarts_within_limit() {
        let config = RestartLimiterConfig {
            enabled: true,
            max_restarts: 3,
            window_secs: 60,
            initial_backoff_ms: 100,
            max_backoff_ms: 1000,
            backoff_multiplier: 2.0,
        };
        let mut limiter = RestartLimiter::new(config);

        // Should allow up to max_restarts
        for _ in 0..3 {
            assert!(limiter.can_restart().is_ok());
            let _ = limiter.record_restart();
        }

        // Should fail on the next attempt
        let result = limiter.can_restart();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.attempts, 3);
        assert_eq!(err.max_restarts, 3);
    }

    #[test]
    fn backoff_grows_exponentially() {
        let config = RestartLimiterConfig {
            enabled: true,
            max_restarts: 10,
            window_secs: 60,
            initial_backoff_ms: 100,
            max_backoff_ms: 10_000,
            backoff_multiplier: 2.0,
        };
        let mut limiter = RestartLimiter::new(config);

        // Each restart should double the backoff
        let backoff1 = limiter.record_restart();
        assert_eq!(backoff1, Duration::from_millis(100));

        let backoff2 = limiter.record_restart();
        assert_eq!(backoff2, Duration::from_millis(200));

        let backoff3 = limiter.record_restart();
        assert_eq!(backoff3, Duration::from_millis(400));

        let backoff4 = limiter.record_restart();
        assert_eq!(backoff4, Duration::from_millis(800));
    }

    #[test]
    fn backoff_is_capped_at_max() {
        let config = RestartLimiterConfig {
            enabled: true,
            max_restarts: 20,
            window_secs: 60,
            initial_backoff_ms: 1000,
            max_backoff_ms: 5000,
            backoff_multiplier: 2.0,
        };
        let mut limiter = RestartLimiter::new(config);

        // First restart: 1000ms
        let _ = limiter.record_restart();
        // Second restart: 2000ms
        let _ = limiter.record_restart();
        // Third restart: 4000ms
        let _ = limiter.record_restart();
        // Fourth restart: would be 8000ms, but capped at 5000ms
        let backoff4 = limiter.record_restart();
        assert_eq!(backoff4, Duration::from_millis(5000));

        // Fifth restart: still capped at 5000ms
        let backoff5 = limiter.record_restart();
        assert_eq!(backoff5, Duration::from_millis(5000));
    }

    #[test]
    fn reset_consecutive_resets_backoff() {
        let config = RestartLimiterConfig {
            enabled: true,
            max_restarts: 10,
            window_secs: 60,
            initial_backoff_ms: 100,
            max_backoff_ms: 10_000,
            backoff_multiplier: 2.0,
        };
        let mut limiter = RestartLimiter::new(config);

        // Build up consecutive restarts
        let _ = limiter.record_restart(); // 100ms
        let _ = limiter.record_restart(); // 200ms
        let _ = limiter.record_restart(); // 400ms

        // Reset
        limiter.reset_consecutive();

        // Next restart should be back to initial
        let backoff = limiter.record_restart();
        assert_eq!(backoff, Duration::from_millis(100));
    }

    #[test]
    fn stats_reflects_current_state() {
        let config = RestartLimiterConfig {
            enabled: true,
            max_restarts: 5,
            window_secs: 60,
            ..Default::default()
        };
        let mut limiter = RestartLimiter::new(config);

        // Initial state
        let stats = limiter.stats();
        assert_eq!(stats.restarts_in_window, 0);
        assert_eq!(stats.consecutive_restarts, 0);
        assert_eq!(stats.window_secs, 60);
        assert_eq!(stats.max_restarts, 5);

        // After some restarts
        let _ = limiter.record_restart();
        let _ = limiter.record_restart();

        let stats = limiter.stats();
        assert_eq!(stats.restarts_in_window, 2);
        assert_eq!(stats.consecutive_restarts, 2);
    }

    #[test]
    fn peek_backoff_does_not_modify_state() {
        let config = RestartLimiterConfig {
            enabled: true,
            max_restarts: 10,
            window_secs: 60,
            initial_backoff_ms: 100,
            max_backoff_ms: 10_000,
            backoff_multiplier: 2.0,
        };
        let limiter = RestartLimiter::new(config);

        // Peek multiple times
        let peek1 = limiter.peek_backoff();
        let peek2 = limiter.peek_backoff();
        let peek3 = limiter.peek_backoff();

        // Should all be the same (initial backoff)
        assert_eq!(peek1, Duration::from_millis(100));
        assert_eq!(peek2, Duration::from_millis(100));
        assert_eq!(peek3, Duration::from_millis(100));

        // State should be unchanged
        assert_eq!(limiter.consecutive_restarts(), 0);
        assert_eq!(limiter.restarts_in_window(), 0);
    }

    #[test]
    fn clone_resets_state() {
        let config = RestartLimiterConfig {
            enabled: true,
            max_restarts: 10,
            window_secs: 60,
            ..Default::default()
        };
        let mut limiter = RestartLimiter::new(config);

        // Build up state
        let _ = limiter.record_restart();
        let _ = limiter.record_restart();
        assert_eq!(limiter.consecutive_restarts(), 2);
        assert_eq!(limiter.restarts_in_window(), 2);

        // Clone should reset state
        let cloned = limiter.clone();
        assert_eq!(cloned.consecutive_restarts(), 0);
        assert_eq!(cloned.restarts_in_window(), 0);
    }

    #[test]
    fn restart_limit_exceeded_error_display() {
        let err = RestartLimitExceeded {
            attempts: 5,
            max_restarts: 5,
            window_secs: 60,
        };
        let display = format!("{err}");
        assert!(display.contains("5 attempts"));
        assert!(display.contains("max 5"));
        assert!(display.contains("60 seconds"));
    }
}
