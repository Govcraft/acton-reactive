//! Provides common types, utilities, and core runtime components for the Acton framework.
//!
//! This module serves as an aggregation point for fundamental building blocks shared across
//! the crate and exposed to users via the prelude. It includes components related to system
//! initialization, runtime management, actor interaction, message brokering, and internal
//! type definitions.
//!
//! # Key Re-exported Components:
//!
//! *   [`ActonApp`]: The entry point for initializing the Acton system.
//! *   [`ActorRuntime`]: Represents the active Acton runtime environment, used for managing
//!     top-level actors and system shutdown.
//! *   [`ActorHandle`]: The primary interface for interacting with individual actors (sending
//!     messages, stopping, supervising).
//! *   [`Broker`]: The central publish-subscribe message broker implementation.
//! *   [`ActorReply`]: A utility struct for creating standard return types for message handlers.
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
pub use broker::Broker;
pub use actor_handle::ActorHandle;
pub use actor_reply::ActorReply;
pub use actor_runtime::ActorRuntime;
pub use config::ActonConfig;

// --- Crate-Internal Re-exports ---
pub use types::*; // Re-export all types from the internal `types` module
pub use crate::message::{Envelope, MessageError, OutboundEnvelope}; // Used by common components

// --- Submodules ---

/// Defines common internal type aliases and structs.
mod types;

/// Defines the `ActonApp` entry point for system initialization.
mod acton;
/// Defines the internal state (`ActonInner`) of the runtime.
mod acton_inner;
/// Defines the `ActorHandle` for actor interaction.
mod actor_handle;
/// Defines the `Broker` implementation.
mod broker;
/// Defines the `ActorRuntime` for managing the system.
mod actor_runtime;
/// Defines the `ActorReply` utility.
mod actor_reply;
/// Defines the configuration system for the Acton framework.
pub mod config;

/// IPC (Inter-Process Communication) support for external process messaging.
///
/// This module provides type registration and serialization infrastructure
/// for sending messages across process boundaries.
///
/// Only available when the `ipc` feature is enabled.
#[cfg(feature = "ipc")]
pub mod ipc;
