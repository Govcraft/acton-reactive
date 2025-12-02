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
use std::fmt::Debug;

use acton_ern::Ern;

use crate::actor::{RestartPolicy, TerminationReason};

/// Represents system-level signals used to manage actor lifecycles.
///
/// These signals are distinct from regular application messages and are typically
/// handled internally by the Acton framework or specific actor implementations
/// to control behavior like termination.
///
/// This enum is marked `#[non_exhaustive]` to indicate that more signal types
/// may be added in future versions without constituting a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)] // Added PartialEq, Eq for potential use
#[non_exhaustive]
pub enum SystemSignal {
    /// Instructs an actor to initiate a graceful shutdown.
    ///
    /// Upon receiving `Terminate`, an actor should:
    /// 1. Stop accepting new work (if applicable).
    /// 2. Complete any in-progress tasks.
    /// 3. Signal its children to terminate.
    /// 4. Wait for children to terminate.
    /// 5. Clean up its own resources.
    /// 6. Stop its message processing loop.
    ///
    /// The exact shutdown sequence is managed by the actor's `wake` loop and
    /// the `stop` method on its [`ActorHandle`](crate::common::ActorHandle).
    Terminate,
    // Other potential signals (commented out in original code):
    // Wake, Recreate, Suspend, Resume, Supervise, Watch, Unwatch, Failed,
}

/// Notification sent to a parent actor when one of its children terminates.
///
/// This message is sent by the child actor's `wake` loop to its parent,
/// providing all the information needed for the parent to decide whether
/// to restart the child based on its [`RestartPolicy`] and the
/// [`TerminationReason`].
///
/// # Supervision Flow
///
/// 1. Child actor terminates (normally, panic, or inbox closed)
/// 2. Child sends `ChildTerminated` to parent (if parent exists)
/// 3. Parent's supervision handler evaluates restart policy
/// 4. Parent may restart the child or take other action
#[derive(Debug, Clone)]
pub struct ChildTerminated {
    /// The unique identifier of the child actor that terminated.
    pub child_id: Ern,
    /// The reason the child terminated.
    pub reason: TerminationReason,
    /// The restart policy configured for this child.
    pub restart_policy: RestartPolicy,
}

impl ChildTerminated {
    /// Creates a new `ChildTerminated` notification.
    #[must_use]
    pub const fn new(child_id: Ern, reason: TerminationReason, restart_policy: RestartPolicy) -> Self {
        Self {
            child_id,
            reason,
            restart_policy,
        }
    }
}
