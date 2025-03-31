//! Defines the core components for creating, configuring, and managing agents.
//!
//! This module provides the fundamental building blocks for actors (referred to as agents
//! within the Acton framework). It encapsulates the agent's lifecycle, state management,
//! and configuration.
//!
//! # Key Components
//!
//! *   [`ManagedAgent`]: The central struct representing an agent's runtime state machine.
//!     It manages the agent's lifecycle (e.g., `Idle`, `Started`), message handling,
//!     and interaction with the Acton system.
//! *   [`AgentConfig`]: A structure holding the necessary configuration parameters
//!     to initialize a new agent, such as its unique identifier (`Ern`) and optional
//!     references to its parent supervisor and the system message broker.
//! *   [`Idle`]: A type-state marker indicating that a `ManagedAgent` has been configured
//!     but has not yet started its main processing loop.
//! *   [`Started`]: A type-state marker indicating that a `ManagedAgent` is actively
//!     running and processing messages.
//!
//! The primary interaction point for creating and managing agents often involves
//! using [`AgentConfig`] to set up initial parameters and then transitioning a
//! [`ManagedAgent`] from the [`Idle`] state to the [`Started`] state.

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
pub use agent_config::AgentConfig;
pub use managed_agent::Idle;
pub use managed_agent::ManagedAgent;
pub use managed_agent::started::Started; // Note: `Started` is defined within a submodule

/// Contains the `ManagedAgent` struct and its state-specific implementations (`Idle`, `Started`).
mod managed_agent;

/// Contains the `AgentConfig` struct for agent initialization.
mod agent_config;
