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

use acton_ern::Ern;
use acton_ern::ErnParser;

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
    /// The supervision strategy recorded for this actor.
    /// Defaults to `SupervisionStrategy::OneForOne`.
    ///
    /// NOTE: This value is recorded but never read by the runtime; see
    /// <https://github.com/govcraft/acton-reactive/issues/7>.
    supervision_strategy: SupervisionStrategy,
    /// Optional restart limiter configuration recorded for this actor.
    ///
    /// NOTE: This value is recorded but never read by the runtime; see
    /// <https://github.com/govcraft/acton-reactive/issues/7>.
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
    /// The policy is delivered to the parent inside the
    /// [`ChildTerminated`](crate::message::ChildTerminated) notification when this
    /// actor terminates, so the parent's handler can decide whether to restart it:
    /// - [`RestartPolicy::Permanent`]: Restart is warranted (except during parent shutdown)
    /// - [`RestartPolicy::Temporary`]: Restart is never warranted
    /// - [`RestartPolicy::Transient`]: Restart is warranted only on abnormal termination
    ///   (panic, inbox closed)
    ///
    /// The framework itself does not restart actors automatically; the parent applies
    /// the policy manually (typically via [`RestartPolicy::should_restart`] or
    /// [`SupervisionStrategy::decide`]).
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

    /// Creates a configuration for a child whose identity is derived from its
    /// parent and its name.
    ///
    /// The child's `Ern` is `parent.add_part(name)`, which yields the same
    /// identifier every time for a given parent and name. That is what lets a
    /// supervisor recreate a child without its identity drifting, and what makes
    /// two children of the same parent sharing a name a genuine collision.
    ///
    /// This is deliberately **not** how [`new`](Self::new) or
    /// [`new_with_name`](Self::new_with_name) build identifiers. Those mint a
    /// fresh root carrying a generated `UUIDv7` suffix, so repeated calls with
    /// the same name produce different actors. Both keep that behavior
    /// unchanged; only supervised children built from a blueprint use this.
    ///
    /// # Scope of the guarantee
    ///
    /// Deterministic **relative to a given parent within one process run**. The
    /// parent's own root still carries a generated suffix, so the full
    /// identifier is not reproducible across processes.
    ///
    /// # Errors
    ///
    /// Returns an error if `name` is not a valid identifier segment.
    pub fn for_supervised_child(
        name: impl Into<String>,
        parent: ParentRef,
        broker: Option<BrokerRef>,
    ) -> anyhow::Result<Self> {
        // The parent's `Ern` is used directly rather than round-tripped through
        // its string form, which would not reproduce the same identifier.
        let id = parent.id().add_part(name.into())?;
        Ok(Self {
            id,
            broker,
            parent: Some(parent),
            inbox_capacity: None,
            restart_policy: RestartPolicy::default(),
            supervision_strategy: SupervisionStrategy::default(),
            restart_limiter_config: None,
        })
    }

    /// Returns a clone of the actor's resolved unique identifier (`Ern`).
    ///
    /// Resolved means final: for a child, this is the full hierarchical
    /// identifier the actor will be created with, not the base name it was
    /// built from.
    #[inline]
    #[must_use]
    pub fn id(&self) -> Ern {
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

    /// Records a supervision strategy for this actor.
    ///
    /// **This method records intent only and has no runtime effect.** The framework
    /// never reads the recorded strategy and never restarts child actors
    /// automatically. When a supervised child terminates, the parent only receives
    /// a [`ChildTerminated`](crate::message::ChildTerminated) notification; acting
    /// on it is up to you.
    ///
    /// To apply a strategy, register a `mutate_on::<ChildTerminated>` handler on
    /// the parent and call [`SupervisionStrategy::decide`] yourself:
    ///
    /// ```rust,ignore
    /// use acton_reactive::prelude::*;
    ///
    /// let strategy = SupervisionStrategy::OneForOne;
    /// supervisor.mutate_on::<ChildTerminated>(move |actor, ctx| {
    ///     let notification = ctx.message().clone();
    ///     match strategy.decide(&notification, 0) {
    ///         SupervisionDecision::RestartChild => {
    ///             // Re-create and re-supervise the failed child here.
    ///         }
    ///         _ => { /* NoRestart, RestartAll, RestartFrom, Escalate */ }
    ///     }
    ///     Reply::ready()
    /// });
    /// ```
    ///
    /// See <https://github.com/govcraft/acton-reactive/issues/7> for the status of
    /// full supervision integration.
    ///
    /// # Arguments
    ///
    /// * `strategy` - The supervision strategy to record for this actor.
    ///
    /// # Returns
    ///
    /// Returns `self` for method chaining.
    #[must_use]
    #[deprecated(
        note = "records intent only and has no runtime effect; the framework never restarts \
                actors automatically. Register `mutate_on::<ChildTerminated>` on the parent and \
                call `SupervisionStrategy::decide()` yourself. \
                See https://github.com/govcraft/acton-reactive/issues/7"
    )]
    pub const fn with_supervision_strategy(mut self, strategy: SupervisionStrategy) -> Self {
        self.supervision_strategy = strategy;
        self
    }

    /// Returns the supervision strategy for this actor.
    #[inline]
    pub(crate) const fn supervision_strategy(&self) -> SupervisionStrategy {
        self.supervision_strategy
    }

    /// Records a restart limiter configuration for this actor.
    ///
    /// **This method records intent only and has no runtime effect.** The framework
    /// never reads the recorded configuration and never restarts child actors
    /// automatically, so no restart limiting or backoff is applied on your behalf.
    ///
    /// To limit restarts manually, keep a [`RestartLimiter`](crate::actor::RestartLimiter)
    /// in the supervising actor's state and consult it from a
    /// `mutate_on::<ChildTerminated>` handler before restarting a child:
    ///
    /// ```rust,ignore
    /// use acton_reactive::prelude::*;
    ///
    /// supervisor.mutate_on::<ChildTerminated>(|actor, _ctx| {
    ///     match actor.model.limiter.can_restart() {
    ///         Ok(()) => {
    ///             let backoff = actor.model.limiter.record_restart();
    ///             // Sleep for `backoff`, then re-create and re-supervise the child.
    ///         }
    ///         Err(exceeded) => {
    ///             // Limit hit: stop restarting and escalate or alert instead.
    ///         }
    ///     }
    ///     Reply::ready()
    /// });
    /// ```
    ///
    /// See <https://github.com/govcraft/acton-reactive/issues/7> for the status of
    /// full supervision integration.
    ///
    /// # Arguments
    ///
    /// * `config` - The [`RestartLimiterConfig`] specifying limits and backoff parameters.
    ///
    /// # Returns
    ///
    /// Returns `self` for method chaining.
    #[must_use]
    #[deprecated(
        note = "records intent only and has no runtime effect; the framework never reads this \
                configuration. Keep a `RestartLimiter` in the supervising actor's state and \
                consult it from a `mutate_on::<ChildTerminated>` handler. \
                See https://github.com/govcraft/acton-reactive/issues/7"
    )]
    pub const fn with_restart_limiter(mut self, config: RestartLimiterConfig) -> Self {
        self.restart_limiter_config = Some(config);
        self
    }

    /// Returns the optional restart limiter configuration recorded for this actor.
    ///
    /// Not yet read by the runtime; retained for the eventual supervision
    /// integration tracked in <https://github.com/govcraft/acton-reactive/issues/7>.
    #[inline]
    #[allow(dead_code)] // Reserved for the supervision integration tracked in issue #7
    pub(crate) const fn restart_limiter_config(&self) -> Option<&RestartLimiterConfig> {
        self.restart_limiter_config.as_ref()
    }
}

#[cfg(test)]
mod supervised_child_identity_tests {
    use super::*;
    use crate::common::ActorHandle;

    fn parent_handle() -> ActorHandle {
        let parent_ern = Ern::with_root("pool").expect("'pool' is a valid Ern root");
        let (outbox, _inbox) = tokio::sync::mpsc::channel(8);
        ActorHandle::new(parent_ern, outbox)
    }

    #[test]
    fn the_same_parent_and_name_always_yield_the_same_identifier() {
        // What makes a blueprint child's identity survive a restart, and what
        // makes a name collision under one parent a real collision.
        let parent = parent_handle();

        let first = ActorConfig::for_supervised_child("worker", parent.clone(), None)
            .expect("'worker' is a valid name");
        let second = ActorConfig::for_supervised_child("worker", parent, None)
            .expect("'worker' is a valid name");

        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn different_names_under_one_parent_stay_distinct() {
        let parent = parent_handle();

        let first = ActorConfig::for_supervised_child("reader", parent.clone(), None)
            .expect("valid name");
        let second = ActorConfig::for_supervised_child("writer", parent, None)
            .expect("valid name");

        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn the_same_name_under_different_parents_stays_distinct() {
        let first = ActorConfig::for_supervised_child("worker", parent_handle(), None)
            .expect("valid name");
        let second = ActorConfig::for_supervised_child("worker", parent_handle(), None)
            .expect("valid name");

        assert_ne!(
            first.id(),
            second.id(),
            "each parent has its own generated root"
        );
    }

    #[test]
    fn the_child_identifier_reads_as_the_parent_then_the_name() {
        let parent = parent_handle();
        let parent_id = parent.id();

        let config = ActorConfig::for_supervised_child("worker", parent, None)
            .expect("valid name");
        let child_id = config.id().to_string();

        assert!(child_id.starts_with(&parent_id.to_string()), "{child_id}");
        assert!(child_id.ends_with("worker"), "{child_id}");
    }

    #[test]
    fn the_ordinary_constructors_keep_minting_fresh_identifiers() {
        // Guards the narrow scope of the change: only the blueprint path is
        // deterministic.
        let first = ActorConfig::new_with_name("worker").expect("valid name");
        let second = ActorConfig::new_with_name("worker").expect("valid name");

        assert_ne!(first.id(), second.id());
    }
}
