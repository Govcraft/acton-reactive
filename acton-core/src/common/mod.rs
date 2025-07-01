//! Provides common types, utilities, and core runtime components for the Acton framework.
//!
//! This module serves as an aggregation point for fundamental building blocks shared across
//! the `acton-core` crate and potentially exposed to users via the prelude. It includes
//! components related to system initialization, runtime management, agent interaction,
//! message brokering, and internal type definitions.
//!
//! # Key Re-exported Components:
//!
//! *   [`ActonApp`]: The entry point for initializing the Acton system.
//! *   [`AgentRuntime`]: Represents the active Acton runtime environment, used for managing
//!     top-level agents and system shutdown.
//! *   [`AgentHandle`]: The primary interface for interacting with individual agents (sending
//!     messages, stopping, supervising).
//! *   [`AgentBroker`]: The central publish-subscribe message broker implementation.
//! *   [`AgentReply`]: A utility struct for creating standard return types for message handlers.
//!
//! Internal types and submodules handle the implementation details for these components.

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

// --- Public Re-exports ---
pub use acton::ActonApp;
pub use agent_broker::AgentBroker;
pub use agent_handle::AgentHandle;
pub use agent_reply::AgentReply;
pub use agent_runtime::AgentRuntime;

// --- Crate-Internal Re-exports ---
pub(crate) use types::*; // Re-export all types from the internal `types` module
pub(crate) use crate::message::{Envelope, MessageError, OutboundEnvelope}; // Used by common components

// --- Submodules ---

/// Defines common internal type aliases and structs.
mod types;

/// Defines the `ActonApp` entry point for system initialization.
mod acton;
/// Defines the internal state (`ActonInner`) of the runtime.
mod acton_inner;
/// Defines the `AgentHandle` for agent interaction.
mod agent_handle;
/// Defines the `AgentBroker` implementation.
mod agent_broker;
/// Defines the `AgentRuntime` for managing the system.
mod agent_runtime;
/// Defines the `AgentReply` utility.
mod agent_reply;

// pub use crate::pool::LoadBalanceStrategy; // This seems unused currently
