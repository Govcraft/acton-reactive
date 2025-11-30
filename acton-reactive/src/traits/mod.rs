//! Defines the core traits that establish the fundamental contracts of the Acton framework.
//!
//! This module aggregates the essential traits that define the capabilities and interactions
//! within the Acton actor system. These traits ensure composability and provide a clear
//! interface for messages, actor handles, the message broker, and the subscription mechanism.
//!
//! # Key Traits
//!
//! *   [`ActonMessage`]: A marker trait required for all types used as messages within the system.
//!     Ensures messages are `Send`, `Sync`, `Debug`, `Clone`, and support downcasting via `Any`.
//! *   [`ActorHandleInterface`]: Defines the primary asynchronous interface for interacting with
//!     actors via their handles ([`ActorHandle`](crate::common::ActorHandle)), including sending messages,
//!     managing lifecycle, and accessing metadata.
//! *   [`Broadcaster`]: Defines the interface for broadcasting messages throughout the system,
//!     typically implemented by [`ActorHandle`](crate::common::ActorHandle) which delegates to the
//!     central [`Broker`](crate::common::Broker).
//! *   [`Subscriber`]: Defines the interface for accessing the system's message broker handle.
//! *   [`Subscribable`]: Defines the interface for actors to manage their subscriptions to
//!     message types via the broker.

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
pub use acton_message::ActonMessage;
pub use acton_message_reply::ActonMessageReply;
pub use actor_handle_interface::ActorHandleInterface;
pub use broker::Broadcaster;
pub use subscribable::Subscribable;
pub use subscriber::Subscriber;

// --- Submodules ---

/// Defines the [`ActonMessage`] marker trait.
mod acton_message;
mod acton_message_reply;
/// Defines the [`ActorHandleInterface`] trait for actor interaction.
mod actor_handle_interface;
/// Defines the [`Broadcaster`] trait for message broadcasting.
mod broker;
/// Defines the [`Subscribable`] trait for managing subscriptions.
mod subscribable;
/// Defines the [`Subscriber`] trait for accessing the broker.
mod subscriber;
