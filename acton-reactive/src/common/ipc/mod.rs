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
//! This module provides type registration, serialization infrastructure, and
//! Unix Domain Socket (UDS) listener for sending messages across process
//! boundaries. All items in this module are only available when the `ipc`
//! feature is enabled.
//!
//! # Overview
//!
//! External processes cannot directly access Tokio MPSC channels used for internal
//! agent communication. The IPC module bridges this gap by providing:
//!
//! - A type registry for mapping message type names to deserializers
//! - Serializable envelope types for IPC message framing
//! - An agent registry for mapping logical names to agent handles
//! - A Unix Domain Socket listener for external process communication
//! - A wire protocol with length-prefixed framing
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
//! * [`IpcConfig`]: Configuration for IPC listener with XDG-compliant socket paths.
//!
//! * [`IpcListenerHandle`]: Handle for managing the IPC listener lifecycle.
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
//!
//! // Start the IPC listener (Phase 2)
//! let listener_handle = runtime.start_ipc_listener().await?;
//! ```

// --- Public Re-exports ---
// These are intentionally public API items for external consumers

pub use config::IpcConfig;
pub use listener::{
    run as start_listener, socket_exists, socket_is_alive, IpcListenerHandle, IpcListenerStats,
    ShutdownResult,
};
pub use registry::IpcTypeRegistry;

// Subscription manager types - used by external clients for broker forwarding
#[allow(unused_imports)]
pub use subscription_manager::{
    create_push_channel, ConnectionId, PushReceiver, PushSender, SubscriptionManager,
    SubscriptionStats,
};

// IPC types - used by external clients for message serialization
#[allow(unused_imports)]
pub use types::{
    AgentInfo, IpcDiscoverRequest, IpcDiscoverResponse, IpcEnvelope, IpcError,
    IpcPushNotification, IpcResponse, IpcSubscribeRequest, IpcSubscriptionResponse,
    IpcUnsubscribeRequest,
};

// Re-export config types for users who want to customize defaults
pub use config::{RateLimitConfig, ShutdownConfig};

// Assert public API types are constructable - compile-time check
const _: () = {
    fn _assert_config_types_usable() {
        let _rate_limit = RateLimitConfig::default();
        let _shutdown = ShutdownConfig::default();
    }
};

// --- Submodules ---

/// IPC configuration with XDG-compliant socket path handling.
mod config;

/// Unix Domain Socket listener for IPC communication.
mod listener;

/// Wire protocol implementation for IPC message framing.
pub mod protocol;

/// Token bucket rate limiter for IPC connections.
mod rate_limiter;

/// Defines the [`IpcTypeRegistry`] for message type registration.
mod registry;

/// Subscription manager for IPC broker forwarding.
pub mod subscription_manager;

/// Defines IPC-specific types: [`IpcEnvelope`], [`IpcResponse`], [`IpcError`].
mod types;
