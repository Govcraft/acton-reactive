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
// #![forbid(dead_code)] // Commenting out during doc rewrite, can be re-enabled later
// #![forbid(unused_imports)] // Commenting out during doc rewrite

//! # Acton Core (`acton-core`)
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
//! This crate primarily defines the internal building blocks and core traits.
//! The `acton-reactive` crate builds upon `acton-core` to provide a more
//! user-friendly API for developing reactive applications.

/// Internal utilities and structures used throughout the Acton framework.
pub(crate) mod common;

/// Defines the core agent structures and logic.
pub(crate) mod actor;

/// Defines message types and envelopes used for communication.
pub(crate) mod message;

/// Defines core traits used throughout the Acton framework.
pub(crate) mod traits;

/// A prelude module for conveniently importing the most commonly used items.
///
/// This module re-exports essential types, traits, and macros from `acton-core`
/// and dependencies like `acton-ern` and `async-trait`, simplifying the import
/// process for users of the Acton framework, particularly within the
/// `acton-reactive` crate.
///
/// # Re-exports
///
/// *   [`acton_ern::*`](https://docs.rs/acton-ern): All items from the `acton-ern` crate for unique resource naming.
/// *   [`async_trait::async_trait`](https://docs.rs/async-trait/latest/async_trait/attr.async_trait.html): The macro for defining async functions in traits.
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
pub mod prelude {
    pub use acton_ern::*;
    pub use async_trait::async_trait; // Corrected re-export

    pub use crate::actor::{AgentConfig, Idle, ManagedAgent, Started};
    pub use crate::common::{ActonApp, AgentBroker, AgentHandle, AgentReply, AgentRuntime};
    pub use crate::message::{BrokerRequest, BrokerRequestEnvelope, MessageAddress, OutboundEnvelope};
    pub use crate::traits::{ActonMessage, AgentHandleInterface, Broker, Subscribable, Subscriber};
}
