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

//! IPC (Inter-Process Communication) support for acton-reactive.
//!
//! This module provides type registration and serialization infrastructure
//! for sending messages across process boundaries. All items in this module
//! are only available when the `ipc` feature is enabled.
//!
//! # Overview
//!
//! External processes cannot directly access Tokio MPSC channels used for internal
//! agent communication. The IPC module bridges this gap by providing:
//!
//! - A type registry for mapping message type names to deserializers
//! - Serializable envelope types for IPC message framing
//! - An agent registry for mapping logical names to agent handles
//!
//! # Key Components
//!
//! * [`IpcTypeRegistry`]: Central registry for mapping type IDs to serialization handlers.
//!   Message types must be registered before they can be received via IPC.
//!
//! * [`IpcEnvelope`]: The standard envelope format for IPC messages, containing
//!   correlation ID, target agent, message type, and serialized payload.
//!
//! * [`IpcResponse`]: Response envelope format with correlation ID matching and
//!   success/error reporting.
//!
//! * [`IpcError`]: Error types specific to IPC operations.
//!
//! # Example
//!
//! ```rust,ignore
//! use acton_reactive::prelude::*;
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Clone, Debug, Serialize, Deserialize)]
//! struct PriceUpdate {
//!     symbol: String,
//!     price: f64,
//! }
//!
//! // Register the message type for IPC
//! let mut runtime = ActonApp::launch();
//! runtime.ipc_registry().register::<PriceUpdate>("PriceUpdate");
//!
//! // Expose an agent for IPC access
//! let agent = runtime.new_agent::<()>();
//! let handle = agent.start().await;
//! runtime.ipc_expose("price_service", handle);
//! ```

// --- Public Re-exports ---
pub use registry::IpcTypeRegistry;
pub use types::{IpcEnvelope, IpcError, IpcResponse};

// --- Submodules ---

/// Defines the [`IpcTypeRegistry`] for message type registration.
mod registry;

/// Defines IPC-specific types: [`IpcEnvelope`], [`IpcResponse`], [`IpcError`].
mod types;
