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

// The original file had no methods, but if an `as_str` or similar were needed:
// impl SystemSignal {
//     /// Returns a string representation of the signal variant.
//     pub fn as_str(&self) -> &'static str {
//         match self {
//             SystemSignal::Terminate => "Terminate",
//         }
//     }
// }
