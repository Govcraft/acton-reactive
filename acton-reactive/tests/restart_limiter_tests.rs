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

//! Integration tests for restart limiter with exponential backoff.

use std::time::Duration;

use acton_reactive::prelude::*;

/// Tests that the default restart limiter config has sensible values.
#[tokio::test]
async fn test_default_restart_limiter_config() {
    let config = RestartLimiterConfig::default();
    assert!(config.enabled);
    assert_eq!(config.max_restarts, 5);
    assert_eq!(config.window_secs, 60);
    assert_eq!(config.initial_backoff_ms, 100);
    assert_eq!(config.max_backoff_ms, 30_000);
    assert!((config.backoff_multiplier - 2.0).abs() < f64::EPSILON);
}

/// Tests creating a disabled restart limiter configuration.
#[tokio::test]
async fn test_disabled_restart_limiter_config() {
    let config = RestartLimiterConfig::disabled();
    assert!(!config.enabled);
}

/// Tests that `RestartLimiter` allows restarts within the limit.
#[tokio::test]
async fn test_limiter_allows_restarts_within_limit() {
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
}

/// Tests that backoff grows exponentially.
#[tokio::test]
async fn test_backoff_grows_exponentially() {
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

/// Tests that backoff is capped at the maximum.
#[tokio::test]
async fn test_backoff_capped_at_max() {
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

/// Tests that resetting consecutive restarts resets backoff.
#[tokio::test]
async fn test_reset_consecutive_resets_backoff() {
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

/// Tests that stats reflect current limiter state.
#[tokio::test]
async fn test_stats_reflects_state() {
    let config = RestartLimiterConfig {
        enabled: true,
        max_restarts: 5,
        window_secs: 60,
        initial_backoff_ms: 100,
        max_backoff_ms: 10_000,
        backoff_multiplier: 2.0,
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

/// Tests that disabled limiter allows unlimited restarts.
#[tokio::test]
async fn test_disabled_limiter_allows_unlimited_restarts() {
    let config = RestartLimiterConfig::disabled();
    let mut limiter = RestartLimiter::new(config);

    // Should always succeed when disabled
    for _ in 0..100 {
        assert!(limiter.can_restart().is_ok());
        let _ = limiter.record_restart();
    }
}

/// Tests the `RestartLimitExceeded` error display.
#[tokio::test]
async fn test_restart_limit_exceeded_display() {
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

/// Tests that `ActorConfig` can be configured with a restart limiter.
#[tokio::test]
async fn test_actor_config_with_restart_limiter() -> anyhow::Result<()> {
    let _config = ActorConfig::new(Ern::with_root("worker")?, None, None)?
        .with_restart_limiter(RestartLimiterConfig {
            enabled: true,
            max_restarts: 3,
            window_secs: 30,
            initial_backoff_ms: 100,
            max_backoff_ms: 5000,
            backoff_multiplier: 2.0,
        });

    Ok(())
}

/// Tests that `peek_backoff` doesn't modify state.
#[tokio::test]
async fn test_peek_backoff_does_not_modify_state() {
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

/// Tests that cloning a limiter resets its state.
#[tokio::test]
async fn test_clone_resets_state() {
    let config = RestartLimiterConfig {
        enabled: true,
        max_restarts: 10,
        window_secs: 60,
        initial_backoff_ms: 100,
        max_backoff_ms: 10_000,
        backoff_multiplier: 2.0,
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

/// Tests configuration helper methods.
#[tokio::test]
async fn test_config_helper_methods() {
    let config = RestartLimiterConfig {
        enabled: true,
        max_restarts: 5,
        window_secs: 60,
        initial_backoff_ms: 100,
        max_backoff_ms: 5000,
        backoff_multiplier: 2.0,
    };

    assert_eq!(config.window_duration(), Duration::from_secs(60));
    assert_eq!(config.initial_backoff(), Duration::from_millis(100));
    assert_eq!(config.max_backoff(), Duration::from_millis(5000));
}
