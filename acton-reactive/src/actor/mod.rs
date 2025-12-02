//! Defines the core components for creating, configuring, and managing actors.
//!
//! This module provides the fundamental building blocks for actors within the Acton
//! framework. It encapsulates the actor's lifecycle, state management, and configuration.
//!
//! # Key Components
//!
//! *   [`ManagedActor`]: The central struct representing an actor's runtime state machine.
//!     It manages the actor's lifecycle (e.g., `Idle`, `Started`), message handling,
//!     and interaction with the Acton system.
//! *   [`ActorConfig`]: A structure holding the necessary configuration parameters
//!     to initialize a new actor, such as its unique identifier (`Ern`) and optional
//!     references to its parent supervisor and the system message broker.
//! *   [`Idle`]: A type-state marker indicating that a `ManagedActor` has been configured
//!     but has not yet started its main processing loop.
//! *   [`Started`]: A type-state marker indicating that a `ManagedActor` is actively
//!     running and processing messages.
//!
//! The primary interaction point for creating and managing actors often involves
//! using [`ActorConfig`] to set up initial parameters and then transitioning a
//! [`ManagedActor`] from the [`Idle`] state to the [`Started`] state.

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

// Re-export key types for easier access within the crate and potentially the prelude.
pub use actor_config::ActorConfig;
pub use managed_actor::Idle;
pub use managed_actor::ManagedActor;
pub use managed_actor::started::Started; // Note: `Started` is defined within a submodule
pub use restart_limiter::{RestartLimiter, RestartLimiterConfig, RestartLimitExceeded, RestartStats};
pub use restart_policy::{RestartPolicy, TerminationReason};
pub use supervision::{SupervisionDecision, SupervisionStrategy};

/// Contains the `ManagedActor` struct and its state-specific implementations (`Idle`, `Started`).
mod managed_actor;

/// Contains the `ActorConfig` struct for actor initialization.
mod actor_config;

/// Contains restart limiters with exponential backoff.
mod restart_limiter;

/// Contains restart policies for supervised actors.
mod restart_policy;

/// Contains supervision strategies for managing child actor restarts.
mod supervision;
