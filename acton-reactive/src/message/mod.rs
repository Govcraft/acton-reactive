//! Defines core message structures, addressing, envelopes, and system signals.
//!
//! This module provides the necessary components for agent communication within the
//! Acton framework. It includes types for identifying agents, wrapping messages
//! for transmission, interacting with the message broker, and sending system-level
//! control signals.
//!
//! # Key Components
//!
//! *   [`MessageAddress`]: Represents the unique, addressable endpoint of an agent,
//!     combining its ID (`Ern`) and its inbox channel sender.
//! *   [`OutboundEnvelope`]: A wrapper for messages being sent *from* an agent,
//!     containing routing information like sender and recipient addresses.
//! *   [`BrokerRequest`]: A message wrapper specifically for requests intended to be
//!     broadcast via the system [`AgentBroker`](crate::common::AgentBroker).
//! *   [`BrokerRequestEnvelope`]: A specialized envelope used internally by the broker
//!     to distribute broadcast messages efficiently.
//! *   [`SystemSignal`]: An enum representing control signals used for managing agent
//!     lifecycles (e.g., `Terminate`).
//!
//! Internal submodules handle implementation details like the internal `Envelope`
//! structure used for channel transmission and messages related to broker subscriptions.

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
pub use broker_request::BrokerRequest;
pub use broker_request_envelope::BrokerRequestEnvelope;
pub use message_address::MessageAddress;
pub use outbound_envelope::OutboundEnvelope;
pub use signal::SystemSignal;

// --- Crate-Internal Re-exports ---
pub use envelope::Envelope;
pub use message_context::MessageContext;
pub use message_error::MessageError;
pub use subscribe_broker::SubscribeBroker;
pub use unsubscribe_broker::UnsubscribeBroker;

// --- Submodules ---

/// Defines [`BrokerRequest`].
mod broker_request;
/// Defines [`BrokerRequestEnvelope`].
mod broker_request_envelope;
/// Defines the internal `Envelope` used for channel communication.
mod envelope;
/// Defines [`MessageContext`] passed to message handlers.
mod message_context;
/// Defines [`MessageError`].
mod message_error;
/// Defines [`OutboundEnvelope`].
mod outbound_envelope;
/// Defines [`MessageAddress`].
mod message_address;
/// Defines [`SystemSignal`].
mod signal;
/// Defines [`SubscribeBroker`] message.
mod subscribe_broker;
/// Defines [`UnsubscribeBroker`] message.
mod unsubscribe_broker;
