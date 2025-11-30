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

#![forbid(unsafe_code)]
#![forbid(missing_docs)] // Keep this to enforce coverage

//! # Acton Reactive
//!
//! This crate provides the foundational components for the Acton asynchronous
//! agent system, built on top of Tokio. It establishes a robust, message-passing
//! framework with clear separation of concerns for agent state, runtime management,
//! communication, and lifecycle.
//!
//! ## Key Concepts
//!
//! - **Agents (`ManagedAgent`)**: Core computational units wrapping user-defined state
//!   and logic, managed by the runtime.
//! - **Handles (`AgentHandle`)**: External references for interacting with agents
//!   (sending messages, stopping, supervising).
//! - **Messaging**: Asynchronous communication via Tokio MPSC channels, using
//!   messages implementing the `ActonMessage` trait.
//! - **Broker (`AgentBroker`)**: Central publish-subscribe mechanism for topic-based
//!   message distribution.
//! - **Lifecycle & Supervision**: Type-state pattern (`Idle`, `Started`) for agents,
//!   lifecycle hooks, and hierarchical supervision.
//! - **Runtime (`AgentRuntime`)**: Manages the overall system, including agent
//!   creation and shutdown.
//! - **Traits**: Core interfaces (`AgentHandleInterface`, `Broker`, `Subscriber`, etc.)
//!   define the framework's capabilities.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use acton_reactive::prelude::*;
//!
//! #[acton_message]
//! struct MyMessage {
//!     content: String,
//! }
//! ```

/// Internal utilities and structures used throughout the Acton framework.
pub(crate) mod common;

/// Defines the core agent structures and logic.
pub(crate) mod actor;

/// Defines message types and envelopes used for communication.
pub(crate) mod message;

/// Defines core traits used throughout the Acton framework.
pub(crate) mod traits;

/// IPC (Inter-Process Communication) module for external process integration.
///
/// This module provides Unix Domain Socket communication infrastructure for
/// sending messages to agents from external processes.
///
/// This module provides all the public IPC types and functions for building
/// IPC clients and working with the IPC infrastructure.
///
/// # Feature Gate
///
/// This module is only available when the `ipc` feature is enabled.
#[cfg(feature = "ipc")]
pub mod ipc {
    pub use crate::common::ipc::{
        socket_exists, socket_is_alive, start_listener, AgentInfo, IpcConfig,
        IpcDiscoverRequest, IpcDiscoverResponse, IpcEnvelope, IpcError, IpcListenerHandle,
        IpcListenerStats, IpcPushNotification, IpcResponse, IpcStreamFrame, IpcSubscribeRequest,
        IpcSubscriptionResponse, IpcTypeRegistry, IpcUnsubscribeRequest, ProtocolCapabilities,
        ProtocolVersionInfo, ShutdownResult,
    };

    /// Wire protocol for IPC message framing.
    ///
    /// This module provides functions for reading and writing IPC messages
    /// using the length-prefixed binary wire protocol.
    pub mod protocol {
        pub use crate::common::ipc::protocol::{
            is_discover, is_heartbeat, is_stream, is_subscribe, is_unsubscribe, read_envelope,
            read_frame, read_response, write_discover, write_discover_with_format,
            write_discovery_response, write_discovery_response_with_format, write_envelope,
            write_envelope_with_format, write_frame, write_heartbeat, write_push_with_format,
            write_response, write_response_with_format, write_stream_frame,
            write_stream_frame_with_format, write_subscribe_with_format,
            write_subscription_response, write_subscription_response_with_format,
            write_unsubscribe_with_format, Format, HEADER_SIZE, HEADER_SIZE_V1, HEADER_SIZE_V2,
            MAX_FRAME_SIZE, MAX_SUPPORTED_VERSION, MIN_SUPPORTED_VERSION, MSG_TYPE_DISCOVER,
            MSG_TYPE_ERROR, MSG_TYPE_HEARTBEAT, MSG_TYPE_PUSH, MSG_TYPE_REQUEST, MSG_TYPE_RESPONSE,
            MSG_TYPE_STREAM, MSG_TYPE_SUBSCRIBE, MSG_TYPE_UNSUBSCRIBE, PROTOCOL_VERSION,
            ProtocolVersion,
        };
    }
}

/// A prelude module for conveniently importing the most commonly used items.
///
/// This module re-exports essential types, traits, and macros from the Acton
/// framework and dependencies like `acton-ern` and `async-trait`, simplifying
/// the import process for users.
///
/// # Re-exports
///
/// ## Macros (from `acton-macro`)
/// *   [`acton_macro::acton_message`]: Attribute macro for defining Acton messages.
/// *   [`acton_macro::acton_actor`]: Attribute macro for defining Acton actors.
///
/// ## External Crates
/// *   [`acton_ern::*`](https://docs.rs/acton-ern): All items from the `acton-ern` crate for unique resource naming.
/// *   [`async_trait::async_trait`](https://docs.rs/async-trait/latest/async_trait/attr.async_trait.html): The macro for defining async functions in traits.
///
/// ## Core Types
/// *   [`crate::actor::AgentConfig`]: Configuration for creating new agents.
/// *   [`crate::actor::Idle`]: Type-state marker for an agent before it starts.
/// *   [`crate::actor::ManagedAgent`]: The core agent structure managing state and runtime.
/// *   [`crate::actor::Started`]: Type-state marker for a running agent.
/// *   [`crate::common::ActonApp`]: Entry point for initializing the Acton system.
/// *   [`crate::common::AgentBroker`]: The central message broker implementation.
/// *   [`crate::common::AgentHandle`]: Handle for interacting with an agent.
/// *   [`crate::common::AgentReply`]: Utility for creating standard message handler return types.
/// *   [`crate::common::AgentRuntime`]: Represents the initialized Acton runtime.
/// *   [`crate::message::BrokerRequest`]: Wrapper for messages intended for broadcast.
/// *   [`crate::message::BrokerRequestEnvelope`]: Specialized envelope for broadcast messages.
/// *   [`crate::message::MessageAddress`]: Addressable endpoint of an agent.
/// *   [`crate::message::OutboundEnvelope`]: Represents a message prepared for sending.
/// *   [`crate::traits::ActonMessage`]: Marker trait for all valid messages.
/// *   [`crate::traits::AgentHandleInterface`]: Core trait defining agent interaction methods.
/// *   [`crate::traits::Broker`]: Trait defining message broadcasting capabilities.
/// *   [`crate::traits::Subscribable`]: Trait for managing message subscriptions.
/// *   [`crate::traits::Subscriber`]: Trait for accessing the message broker.
///
/// ## IPC Types (requires `ipc` feature)
/// *   [`crate::common::ipc::IpcTypeRegistry`]: Registry for IPC message type deserialization.
/// *   [`crate::common::ipc::IpcEnvelope`]: Envelope format for IPC messages.
/// *   [`crate::common::ipc::IpcResponse`]: Response envelope format for IPC.
/// *   [`crate::common::ipc::IpcError`]: Error types for IPC operations.
/// *   [`crate::common::ipc::IpcConfig`]: Configuration for IPC listener.
/// *   [`crate::common::ipc::IpcListenerHandle`]: Handle for managing IPC listener lifecycle.
/// *   [`crate::common::ipc::IpcListenerStats`]: Statistics for IPC listener.
pub mod prelude {
    // Macros from acton-macro
    pub use acton_macro::*;

    // External crate re-exports
    pub use acton_ern::*;
    pub use async_trait::async_trait;

    // Core types
    pub use crate::actor::{AgentConfig, Idle, ManagedAgent, Started};
    pub use crate::common::{ActonApp, AgentBroker, AgentHandle, AgentReply, AgentRuntime};
    pub use crate::message::{BrokerRequest, BrokerRequestEnvelope, MessageAddress, OutboundEnvelope};
    pub use crate::traits::{ActonMessage, AgentHandleInterface, Broker, Subscribable, Subscriber};

    // IPC types (feature-gated)
    #[cfg(feature = "ipc")]
    pub use crate::common::ipc::{
        IpcConfig, IpcEnvelope, IpcError, IpcListenerHandle, IpcListenerStats, IpcResponse,
        IpcTypeRegistry, ShutdownResult,
    };
}
