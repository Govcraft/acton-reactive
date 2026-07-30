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

//! Supervision of child actors.
//!
//! Every type in this module is re-exported from here, so the public paths
//! [`SupervisionStrategy`] and [`SupervisionDecision`] resolve exactly as they
//! did when this module was a single file.
//!
//! # Layout
//!
//! - [`strategy`] — which children to restart when one terminates
//! - [`registry`] — the value types describing a supervisor's children
//! - [`status`] — what a supervisor publishes about one child
//! - [`escalation`] — what happens once restarting stops working
//! - [`events`] — supervision notifications broadcast over the broker
//! - [`error`] — the errors the subsystem returns

pub use error::SupervisionError;
pub use escalation::Escalation;
pub use events::{ChildRestarted, ChildSupervised, SupervisionEscalated};
pub use registry::{BackoffDelay, ChildIndex, RestartGeneration};
pub use status::{SupervisionState, SupervisionStatus};
pub use strategy::{SupervisionDecision, SupervisionStrategy};

/// Contains the errors returned by the supervision subsystem.
mod error;

/// Contains the policy for what a supervisor does after restarts stop working.
mod escalation;

/// Contains the supervision events broadcast over the system broker.
mod events;

/// Contains the value types describing a supervisor's view of its children.
mod registry;

/// Contains a supervisor's published view of one supervised child.
mod status;

/// Contains the supervision strategies and the decisions they produce.
mod strategy;
