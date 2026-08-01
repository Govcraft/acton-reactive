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

use crate::actor::{RestartLimiterConfig, RestartPolicy, SupervisionStrategy};
use crate::common::{BrokerRef, ParentRef};
use crate::traits::ActorHandleInterface;

/// How deep a chain of supervised children may go.
///
/// A child's identifier is its parent's with one part appended, so this is also
/// the number of parts an actor's [`Ern`] may carry. Reaching it is refused by
/// [`ActorConfig::for_supervised_child`] under this name, rather than surfacing
/// `acton-ern`'s generic message about parts.
///
/// # This value is not free to change
///
/// It must equal the cap `acton-ern` itself enforces. Version 2 hardcodes 10
/// inside `Ern::add_part` and exposes no constant, no accessor, and no
/// `add_part_with_limit` to read it from. Raising this number without an
/// `acton-ern` 3 that raises its own cap would only move which error you get:
/// the check below would stop firing first and `add_part` would refuse the same
/// part anyway, with a worse message.
///
/// `a_child_at_the_depth_limit_is_refused_by_name` fails if that drift happens.
pub const MAX_SUPERVISION_DEPTH: usize = 10;

/// Configuration parameters required to initialize a new actor.
///
/// This struct encapsulates the essential settings for creating an actor instance,
/// including its unique identity, its relationship within the actor hierarchy (parent),
/// and its connection to the system message broker.
///
/// The actor's identity is represented by an [`Ern`](acton_ern::Ern), which supports
/// hierarchical naming. A root actor is built with [`new`](Self::new) or
/// [`new_with_name`](Self::new_with_name); a child is built with
/// [`for_supervised_child`](Self::for_supervised_child), which appends the child's
/// name to its parent's `Ern` as a part.
#[derive(Default, Debug, Clone)]
pub struct ActorConfig {
    /// The actor's resolved unique identifier (`Ern`).
    /// For a child, this is the full hierarchical ID, `<parent-ern>/<name>`.
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
    /// How this actor responds when one of the children it supervises
    /// terminates. Defaults to `SupervisionStrategy::OneForOne`.
    supervision_strategy: SupervisionStrategy,
    /// How many times a child may be restarted and how long to wait between
    /// attempts. `None` means inherit the supervisor's.
    restart_limiter_config: Option<RestartLimiterConfig>,
}

impl ActorConfig {
    /// Creates a configuration for a top-level actor with the identifier you give it.
    ///
    /// The `id` is used as-is. This constructor builds **root** actors only; to
    /// build a child under a supervisor, use
    /// [`for_supervised_child`](Self::for_supervised_child), which derives the
    /// child's identifier from its parent's and its name.
    ///
    /// # Arguments
    ///
    /// * `id` - The identifier (`Ern`) for the actor.
    /// * `broker` - An optional [`BrokerRef`] (handle) to the system message broker.
    #[must_use]
    pub fn new(id: Ern, broker: Option<BrokerRef>) -> Self {
        Self {
            id,
            broker,
            parent: None,
            inbox_capacity: None,
            restart_policy: RestartPolicy::default(),
            supervision_strategy: SupervisionStrategy::default(),
            restart_limiter_config: None,
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
        Ok(Self::new(Ern::with_root(name.into())?, None))
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
    /// Returns an error if `name` is not a valid identifier segment, or if the
    /// parent already sits at [`MAX_SUPERVISION_DEPTH`].
    pub fn for_supervised_child(
        name: impl Into<String>,
        parent: ParentRef,
        broker: Option<BrokerRef>,
    ) -> anyhow::Result<Self> {
        let parent_id = parent.id();
        let depth = parent_id.parts().len();
        if depth >= MAX_SUPERVISION_DEPTH {
            let name = name.into();
            return Err(anyhow::anyhow!(
                "supervision depth limit reached: {parent_id} is already \
                 {depth} levels deep, and the maximum supervision depth is \
                 {MAX_SUPERVISION_DEPTH}, so '{name}' cannot be added beneath it"
            ));
        }

        // The parent's `Ern` is used directly rather than round-tripped through
        // its string form, which would not reproduce the same identifier.
        let id = parent_id.add_part(name.into())?;
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

    /// Sets how this actor responds when one of the children it supervises
    /// terminates.
    ///
    /// The strategy consulted for a failure is the **supervisor's**, not the
    /// child's. That is worth stating outright, because
    /// [`for_supervised_child`](Self::for_supervised_child) builds a *child's*
    /// configuration and a reader may reasonably expect a strategy set there to
    /// govern what happens when that child dies. It does not: what to do about
    /// a failure is the supervising actor's policy.
    ///
    /// # What each one does
    ///
    /// All three are carried out in full, and the supervisor keeps taking
    /// messages throughout.
    ///
    /// [`SupervisionStrategy::OneForOne`] — the default, and therefore what
    /// every unconfigured actor gets — restarts the failed child from its
    /// blueprint after a backoff and leaves its siblings alone.
    ///
    /// [`OneForAll`] and [`RestForOne`] restart a group. The siblings the
    /// strategy names are stopped in **reverse start order**, each one fully
    /// down before the one before it is asked — so a child that depends on an
    /// earlier sibling goes first. The whole group is down before any of it
    /// comes back. One backoff is charged for the whole group, because a group
    /// restart is one supervisory event rather than several.
    ///
    /// The rebuilds are then *requested* in start order. They are not awaited
    /// in start order, and the difference matters if your children have startup
    /// dependencies: each start runs on its own task, because a child's
    /// `before_start` is user code of unbounded duration and waiting for it
    /// would stop the supervisor taking messages. So an earlier sibling is
    /// asked for first but may finish starting after a later one. A child that
    /// cannot come up until a sibling is ready should wait for it rather than
    /// assume start order has done that for it.
    ///
    /// A sibling the supervisor cannot recreate is still **stopped** by a group
    /// restart and simply never comes back. That is deliberate: the point of
    /// [`OneForAll`] is that the children are interdependent, so leaving one
    /// running against a freshly restarted set would expose exactly the
    /// inconsistent state the strategy exists to prevent.
    ///
    /// # Only children with a blueprint
    ///
    /// A strategy governs children registered through
    /// [`supervise_with`](crate::common::ActorHandle::supervise_with) or
    /// [`supervise_deferred`](crate::actor::ManagedActor::supervise_deferred).
    /// A child adopted through the legacy `supervise()` path is one the
    /// supervisor has no recipe for rebuilding, so it is left down exactly as
    /// it is today and no strategy applies to it.
    ///
    /// # If you already hand-rolled this
    ///
    /// Earlier releases had no restart engine and this method's documentation
    /// told you to write a `mutate_on::<ChildTerminated>` handler that restarts
    /// the child yourself. Those handlers still run — the engine's bookkeeping
    /// is additive and does not suppress dispatch — so when you move a child to
    /// `supervise_with` or `supervise_deferred`, **delete the hand-rolled
    /// restart** or that child will come back twice.
    ///
    /// [`OneForAll`]: SupervisionStrategy::OneForAll
    /// [`RestForOne`]: SupervisionStrategy::RestForOne
    ///
    /// # Arguments
    ///
    /// * `strategy` - The supervision strategy this actor applies to its
    ///   children.
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

    /// Sets how many times an actor may be restarted, and how long to wait
    /// between attempts.
    ///
    /// # Which limiter governs a child
    ///
    /// **A child's own setting wins; a child that sets none inherits its
    /// supervisor's.** So this method is meaningful on both sides of the
    /// relationship, and setting it on a child is an override rather than a
    /// no-op: a child that knows it is expensive to rebuild can raise its own
    /// `max_restarts` above what its supervisor would have allowed.
    ///
    /// Each child is held to a limiter of its **own**, never one shared across
    /// siblings, so one child failing repeatedly cannot consume the allowance
    /// of a sibling that has never failed.
    ///
    /// # What it controls
    ///
    /// A child that terminates in a way its [`RestartPolicy`] warrants a
    /// restart from waits out an exponentially growing backoff and is then
    /// rebuilt from its blueprint. A child that exceeds `max_restarts` within
    /// `window_secs` is not restarted again: its supervisor gives up and
    /// publishes [`SupervisionState::Escalated`] with the reason, so anything
    /// waiting on that child stops waiting rather than hanging.
    ///
    /// A child that stays up longer than `window_secs` counts as recovered and
    /// its backoff starts over rather than compounding from its last failure.
    ///
    /// # Only children with a blueprint
    ///
    /// Applies to children registered through
    /// [`supervise_with`](crate::common::ActorHandle::supervise_with) or
    /// [`supervise_deferred`](crate::actor::ManagedActor::supervise_deferred).
    /// A child adopted through the legacy `supervise()` path is never restarted
    /// — the supervisor holds no recipe for rebuilding it — so no allowance is
    /// consulted or consumed for it.
    ///
    /// # If you already hand-rolled this
    ///
    /// Earlier releases had no restart engine and this method's documentation
    /// told you to keep a [`RestartLimiter`](crate::actor::RestartLimiter) in
    /// your supervisor's state and restart children yourself. Those handlers
    /// still run. When you move a child to `supervise_with` or
    /// `supervise_deferred`, **delete the hand-rolled restart** or that child
    /// will come back twice.
    ///
    /// [`SupervisionState::Escalated`]: crate::actor::SupervisionState::Escalated
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

    /// Returns the restart limiter configuration recorded for this actor.
    ///
    /// `None` means this actor expressed no preference, which a supervisor
    /// reads as "inherit mine".
    #[inline]
    pub(crate) const fn restart_limiter_config(&self) -> Option<&RestartLimiterConfig> {
        self.restart_limiter_config.as_ref()
    }
}

#[cfg(test)]
mod supervised_child_identity_tests {
    use super::*;
    use crate::common::ActorHandle;

    fn parent_handle() -> ActorHandle {
        handle_at_depth(0)
    }

    /// A handle whose `Ern` already carries `depth` parts.
    fn handle_at_depth(depth: usize) -> ActorHandle {
        ActorHandle::new(ern_at_depth(depth), tokio::sync::mpsc::channel(8).0)
    }

    /// An `Ern` rooted at "pool" carrying `depth` parts.
    fn ern_at_depth(depth: usize) -> Ern {
        (0..depth).fold(
            Ern::with_root("pool").expect("'pool' is a valid Ern root"),
            |ern, level| {
                ern.add_part(format!("level{level}"))
                    .expect("depth is within what acton-ern allows")
            },
        )
    }

    #[test]
    fn a_child_at_the_depth_limit_is_refused_by_name() {
        let deep = ern_at_depth(MAX_SUPERVISION_DEPTH);

        let error = ActorConfig::for_supervised_child(
            "one_too_many",
            ActorHandle::new(deep.clone(), tokio::sync::mpsc::channel(8).0),
            None,
        )
        .expect_err("the parent is already at the depth limit");

        let message = error.to_string();
        assert!(
            message.contains("supervision depth"),
            "the refusal should name supervision depth rather than acton-ern's \
             generic message about parts, but said: {message}"
        );
        assert!(
            message.contains("one_too_many"),
            "the refusal should name the child that was refused: {message}"
        );

        // `MAX_SUPERVISION_DEPTH` is the dependency's boundary, not merely some
        // number below it: at this same depth acton-ern refuses the part on its
        // own. Raising the constant without an acton-ern whose cap also rises
        // would stop our check firing first, and the assertions above would then
        // see acton-ern's message instead of ours.
        assert!(
            deep.add_part("one_too_many").is_err(),
            "acton-ern's own cap must sit exactly at MAX_SUPERVISION_DEPTH"
        );
    }

    #[test]
    fn a_child_one_below_the_depth_limit_is_still_allowed() {
        let parent = handle_at_depth(MAX_SUPERVISION_DEPTH - 1);

        let config = ActorConfig::for_supervised_child("last_level", parent, None)
            .expect("one level remains beneath the limit");

        assert_eq!(config.id().parts().len(), MAX_SUPERVISION_DEPTH);
        let id = config.id().to_string();
        assert!(id.ends_with("last_level"), "{id}");
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
