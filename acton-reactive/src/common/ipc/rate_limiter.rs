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

//! Token bucket rate limiter for IPC connections.
//!
//! This module provides a per-connection rate limiter using the token bucket algorithm.
//! Tokens are replenished at a configured rate up to a maximum burst capacity.

use std::time::{Duration, Instant};

use super::config::RateLimitConfig;

/// A token bucket rate limiter.
///
/// The token bucket algorithm allows bursts of traffic up to the bucket capacity,
/// while maintaining a sustained rate limit over time. Tokens are continuously
/// replenished at the configured rate.
///
/// # Example
///
/// ```rust,ignore
/// let config = RateLimitConfig::default();
/// let mut limiter = RateLimiter::new(&config);
///
/// // Try to acquire a token
/// if limiter.try_acquire() {
///     // Process the request
/// } else {
///     // Rate limited, retry after limiter.time_until_available()
/// }
/// ```
#[derive(Debug)]
pub struct RateLimiter {
    /// Current number of available tokens.
    tokens: f64,
    /// Maximum tokens (burst capacity).
    capacity: f64,
    /// Token refill rate (tokens per second).
    refill_rate: f64,
    /// Last time tokens were updated.
    last_update: Instant,
    /// Whether rate limiting is enabled.
    enabled: bool,
}

impl RateLimiter {
    /// Create a new rate limiter from configuration.
    #[must_use]
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            tokens: f64::from(config.burst_size),
            capacity: f64::from(config.burst_size),
            refill_rate: f64::from(config.requests_per_second),
            last_update: Instant::now(),
            enabled: config.enabled,
        }
    }

    /// Try to acquire a single token.
    ///
    /// Returns `true` if a token was acquired, `false` if rate limited.
    pub fn try_acquire(&mut self) -> bool {
        if !self.enabled {
            return true;
        }

        self.refill();

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Get the time until a token will be available.
    ///
    /// Returns `Duration::ZERO` if a token is already available or if
    /// rate limiting is disabled.
    #[must_use]
    pub fn time_until_available(&mut self) -> Duration {
        if !self.enabled {
            return Duration::ZERO;
        }

        self.refill();

        if self.tokens >= 1.0 {
            Duration::ZERO
        } else {
            // Calculate time until we have at least 1 token
            let tokens_needed = 1.0 - self.tokens;
            let seconds_needed = tokens_needed / self.refill_rate;
            Duration::from_secs_f64(seconds_needed)
        }
    }

    /// Check if rate limiting is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get the current number of available tokens.
    #[must_use]
    pub fn available_tokens(&mut self) -> f64 {
        self.refill();
        self.tokens
    }

    /// Refill tokens based on elapsed time.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_update).as_secs_f64();
        self.last_update = now;

        // Add tokens based on elapsed time
        self.tokens = elapsed.mul_add(self.refill_rate, self.tokens).min(self.capacity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_initial_burst() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 10,
            burst_size: 5,
        };
        let mut limiter = RateLimiter::new(&config);

        // Should allow burst_size requests immediately
        for _ in 0..5 {
            assert!(limiter.try_acquire(), "Should allow initial burst");
        }

        // Next request should be rate limited
        assert!(!limiter.try_acquire(), "Should rate limit after burst");
    }

    #[test]
    fn test_rate_limiter_disabled() {
        let config = RateLimitConfig {
            enabled: false,
            requests_per_second: 10,
            burst_size: 5,
        };
        let mut limiter = RateLimiter::new(&config);

        // Should allow unlimited requests when disabled
        for _ in 0..100 {
            assert!(limiter.try_acquire(), "Should allow all requests when disabled");
        }
    }

    #[test]
    fn test_rate_limiter_disabled_via_config() {
        let config = RateLimitConfig {
            enabled: false,
            requests_per_second: 10,
            burst_size: 5,
        };
        let mut limiter = RateLimiter::new(&config);
        assert!(!limiter.is_enabled());
        assert!(limiter.try_acquire());
        assert_eq!(limiter.time_until_available(), Duration::ZERO);
    }

    #[test]
    fn test_rate_limiter_time_until_available() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 10,
            burst_size: 1,
        };
        let mut limiter = RateLimiter::new(&config);

        // Use the one available token
        assert!(limiter.try_acquire());

        // Should need to wait
        let wait_time = limiter.time_until_available();
        assert!(wait_time > Duration::ZERO);
        // At 10 tokens/second, should need ~100ms for 1 token
        assert!(wait_time <= Duration::from_millis(150));
    }

    #[test]
    fn test_rate_limiter_refill() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 1000, // Fast refill for testing
            burst_size: 1,
        };
        let mut limiter = RateLimiter::new(&config);

        // Use the one available token
        assert!(limiter.try_acquire());
        assert!(!limiter.try_acquire());

        // Wait a bit for refill (1ms should give us ~1 token at 1000/sec)
        std::thread::sleep(Duration::from_millis(2));

        // Should have refilled
        assert!(limiter.try_acquire());
    }

    #[test]
    fn test_rate_limiter_capacity_limit() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 1000,
            burst_size: 3,
        };
        let mut limiter = RateLimiter::new(&config);

        // Wait to accumulate tokens
        std::thread::sleep(Duration::from_millis(10));

        // Should still be capped at burst_size (3)
        let tokens = limiter.available_tokens();
        assert!(tokens <= 3.0, "Tokens should not exceed capacity");
    }

    #[test]
    fn test_default_config_rate_limiter() {
        let config = RateLimitConfig::default();
        let mut limiter = RateLimiter::new(&config);

        assert!(limiter.is_enabled());
        // Default burst is 50
        for _ in 0..50 {
            assert!(limiter.try_acquire());
        }
    }
}
