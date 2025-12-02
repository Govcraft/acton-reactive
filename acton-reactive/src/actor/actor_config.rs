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

use acton_ern::{Ern, ErnParser};

use crate::actor::{RestartLimiterConfig, RestartPolicy, SupervisionStrategy};
use crate::common::{BrokerRef, ParentRef};
use crate::traits::ActorHandleInterface;

/// Configuration parameters required to initialize a new actor.
///
/// This struct encapsulates the essential settings for creating an actor instance,
/// including its unique identity, its relationship within the actor hierarchy (parent),
/// and its connection to the system message broker.
///
/// The actor's identity is represented by an [`Ern`](acton_ern::Ern), which supports
/// hierarchical naming. If a `parent` actor is specified during configuration, the
/// final `Ern` of the new actor will be derived by appending its base `id` to the
/// parent's `Ern`.
#[derive(Default, Debug, Clone)]
pub struct ActorConfig {
    /// The unique identifier (`Ern`) for the actor.
    /// If created under a parent, this will be the fully resolved hierarchical ID.
    id: Ern,
    /// Optional handle to the system message broker.
    pub(crate) broker: Option<BrokerRef>,
    /// Optional handle to the actor's parent (supervisor).
    parent: Option<ParentRef>,
    /// Optional custom inbox capacity for this actor.
    /// If `None`, uses the global default from configuration.
    inbox_capacity: Option<usize>,
    /// The restart policy for this actor when supervised.
    /// Defaults to `RestartPolicy::Permanent`.
    restart_policy: RestartPolicy,
    /// The supervision strategy for managing child actors.
    /// Defaults to `SupervisionStrategy::OneForOne`.
    supervision_strategy: SupervisionStrategy,
    /// Optional restart limiter configuration for this actor.
    /// When `Some`, the supervisor will use this configuration to limit
    /// restart frequency and apply exponential backoff.
    restart_limiter_config: Option<RestartLimiterConfig>,
}

impl ActorConfig {
    /// Creates a new `ActorConfig` instance, potentially deriving a hierarchical ID.
    ///
    /// This constructor configures a new actor. If a `parent` handle is provided,
    /// the actor's final `id` (`Ern`) is constructed by appending the provided `id`
    /// segment to the parent's `Ern`. If no `parent` is provided, the `id` is used directly.
    ///
    /// # Arguments
    ///
    /// * `id` - The base identifier (`Ern`) for the actor. If `parent` is `Some`, this
    ///   acts as the final segment appended to the parent's ID. If `parent` is `None`,
    ///   this becomes the actor's root ID.
    /// * `parent` - An optional [`ParentRef`] (handle) to the supervising actor.
    /// * `broker` - An optional [`BrokerRef`] (handle) to the system message broker.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing the configured `ActorConfig` instance.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing the parent's ID string into an `Ern` fails when
    /// constructing a hierarchical ID.
    pub fn new(
        id: Ern,
        parent: Option<ParentRef>,
        broker: Option<BrokerRef>,
    ) -> anyhow::Result<Self> {
        if let Some(parent_ref) = parent {
            // Use a different variable name to avoid shadowing
            // Get the parent ERN
            let parent_id = ErnParser::new(parent_ref.id().to_string()).parse()?;
            let child_id = parent_id + id;
            Ok(Self {
                id: child_id,
                broker,
                parent: Some(parent_ref),
                inbox_capacity: None,
                restart_policy: RestartPolicy::default(),
                supervision_strategy: SupervisionStrategy::default(),
                restart_limiter_config: None,
            })
        } else {
            Ok(Self {
                id,
                broker,
                parent, // parent is None here
                inbox_capacity: None,
                restart_policy: RestartPolicy::default(),
                supervision_strategy: SupervisionStrategy::default(),
                restart_limiter_config: None,
            })
        }
    }

    /// Sets a custom inbox capacity for this actor.
    ///
    /// This allows overriding the global default inbox capacity on a per-actor basis.
    /// High-throughput actors may benefit from larger capacities, while low-throughput
    /// actors can use smaller capacities to conserve memory.
    ///
    /// # Arguments
    ///
    /// * `capacity` - The inbox channel capacity for this actor.
    ///
    /// # Returns
    ///
    /// Returns `self` for method chaining.
    #[must_use]
    pub const fn with_inbox_capacity(mut self, capacity: usize) -> Self {
        self.inbox_capacity = Some(capacity);
        self
    }

    /// Sets the restart policy for this actor when supervised.
    ///
    /// The restart policy determines how the supervisor handles actor termination:
    /// - [`RestartPolicy::Permanent`]: Always restart (except during parent shutdown)
    /// - [`RestartPolicy::Temporary`]: Never restart
    /// - [`RestartPolicy::Transient`]: Restart only on abnormal termination (panic, inbox closed)
    ///
    /// # Arguments
    ///
    /// * `policy` - The restart policy to use for this actor.
    ///
    /// # Returns
    ///
    /// Returns `self` for method chaining.
    #[must_use]
    pub const fn with_restart_policy(mut self, policy: RestartPolicy) -> Self {
        self.restart_policy = policy;
        self
    }

    /// Creates a new `ActorConfig` for a top-level actor with a root identifier.
    ///
    /// This is a convenience function for creating an `ActorConfig` for an actor
    /// that has no parent (i.e., it's a root actor in the hierarchy). The provided
    /// `name` is used to create a root [`Ern`](acton_ern::Ern).
    ///
    /// # Arguments
    ///
    /// * `name` - A string-like value that will be used as the root name for the actor's `Ern`.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing the new `ActorConfig` instance with no parent or broker.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the root `Ern` from the provided `name` fails
    /// (e.g., if the name is invalid according to `Ern` rules).
    pub fn new_with_name(name: impl Into<String>) -> anyhow::Result<Self> {
        Self::new(Ern::with_root(name.into())?, None, None)
    }

    /// Returns a clone of the actor's unique identifier (`Ern`).
    #[inline]
    pub(crate) fn id(&self) -> Ern {
        self.id.clone()
    }

    /// Returns a reference to the optional broker handle.
    #[inline]
    pub(crate) const fn get_broker(&self) -> Option<&BrokerRef> {
        self.broker.as_ref()
    }

    /// Returns a reference to the optional parent handle.
    #[inline]
    pub(crate) const fn parent(&self) -> Option<&ParentRef> {
        self.parent.as_ref()
    }

    /// Returns the optional custom inbox capacity for this actor.
    ///
    /// If `None`, the actor should use the global default from configuration.
    #[inline]
    pub(crate) const fn inbox_capacity(&self) -> Option<usize> {
        self.inbox_capacity
    }

    /// Returns the restart policy for this actor.
    #[inline]
    pub(crate) const fn restart_policy(&self) -> RestartPolicy {
        self.restart_policy
    }

    /// Sets the supervision strategy for managing child actors.
    ///
    /// The supervision strategy determines how the supervisor handles child terminations:
    /// - [`SupervisionStrategy::OneForOne`]: Restart only the failed child
    /// - [`SupervisionStrategy::OneForAll`]: Restart all children when one fails
    /// - [`SupervisionStrategy::RestForOne`]: Restart the failed child and all children started after it
    ///
    /// # Arguments
    ///
    /// * `strategy` - The supervision strategy to use for this actor.
    ///
    /// # Returns
    ///
    /// Returns `self` for method chaining.
    #[must_use]
    pub const fn with_supervision_strategy(mut self, strategy: SupervisionStrategy) -> Self {
        self.supervision_strategy = strategy;
        self
    }

    /// Returns the supervision strategy for this actor.
    #[inline]
    pub(crate) const fn supervision_strategy(&self) -> SupervisionStrategy {
        self.supervision_strategy
    }

    /// Sets the restart limiter configuration for this actor.
    ///
    /// The restart limiter controls how frequently an actor can be restarted
    /// and applies exponential backoff between restart attempts:
    /// - Tracks restarts within a sliding time window
    /// - Limits the maximum number of restarts within that window
    /// - Applies exponential backoff delays between restarts
    ///
    /// When the restart limit is exceeded, the supervisor should escalate
    /// the failure to its parent rather than continuing to restart.
    ///
    /// # Arguments
    ///
    /// * `config` - The [`RestartLimiterConfig`] specifying limits and backoff parameters.
    ///
    /// # Returns
    ///
    /// Returns `self` for method chaining.
    #[must_use]
    pub const fn with_restart_limiter(mut self, config: RestartLimiterConfig) -> Self {
        self.restart_limiter_config = Some(config);
        self
    }

    /// Returns the optional restart limiter configuration for this actor.
    ///
    /// Used by the supervision system when creating restart limiters for child actors.
    #[inline]
    #[allow(dead_code)] // Will be used when supervision fully integrates restart limiting
    pub(crate) const fn restart_limiter_config(&self) -> Option<&RestartLimiterConfig> {
        self.restart_limiter_config.as_ref()
    }
}
