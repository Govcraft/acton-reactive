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

//! Defines common internal type aliases and supporting structures used within `acton-core`.
//!
//! This module centralizes type definitions for futures, handlers, channels, and other
//! implementation details to improve code readability and maintainability. It also
//! defines public type aliases for specific uses of [`AgentHandle`].

use std::any::TypeId;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;

use dashmap::DashMap;
use tokio::sync::mpsc::Sender;

use crate::actor::{ManagedAgent, Started};
use crate::common::AgentHandle;
use crate::message::Envelope;

/// Crate-internal: Map storing message handlers (`TypeId` -> `ReactorItem`).
pub(crate) type ReactorMap<ActorEntity> = DashMap<TypeId, ReactorItem<ActorEntity>>;

/// Crate-internal: Enum wrapping different kinds of message/event handlers.
/// Currently only supports `FutureReactor`.
#[allow(dead_code)] // Allow dead_code for potential future variants like SignalReactor
pub(crate) enum ReactorItem<ActorEntity: Default + Send + Debug + 'static> {
    // SignalReactor(Box<SignalHandler<ActorEntity>>),
    /// A handler that processes a message and returns a future.
    FutureReactor(Box<FutureHandler<ActorEntity>>),
}

/// Crate-internal: Type alias for the function signature of a message handler
/// that returns a future.
pub(crate) type FutureHandler<ManagedEntity> = dyn for<'a, 'b> Fn(
    &'a mut ManagedAgent<Started, ManagedEntity>, // Takes mutable agent in Started state
    &'b mut Envelope, // Takes mutable envelope containing the message
) -> FutureBox // Returns a pinned, boxed future
    + Send
    + Sync
    + 'static;

/// Crate-internal: Type alias for a pinned, boxed, dynamically dispatched future
/// with `Output = ()` that is `Send`, `Sync`, and `'static`.
/// This is the required return type for asynchronous message handlers (`act_on`).
pub(crate) type FutureBox = Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>;


/// Crate-internal: Type alias for the sender part of an agent's MPSC channel.
pub(crate) type AgentSender = Sender<Envelope>;

/// Crate-internal: Type alias for the atomic boolean used as a halt signal for agents.
pub(crate) type HaltSignal = AtomicBool;

/// Crate-internal: Type alias for the function signature of an asynchronous lifecycle hook.
pub(crate) type AsyncLifecycleHandler<ManagedEntity> =
    Box<dyn Fn(&ManagedAgent<Started, ManagedEntity>) -> FutureBox + Send + Sync + 'static>;

// --- Public Type Aliases ---

/// A type alias representing a handle ([`AgentHandle`]) specifically for the system message broker.
///
/// This alias provides semantic clarity when a handle refers to the central [`AgentBroker`].
pub type BrokerRef = AgentHandle;

/// A type alias representing a handle ([`AgentHandle`]) specifically for an agent's parent (supervisor).
///
/// This alias provides semantic clarity in hierarchical agent structures, indicating that
/// the handle refers to the agent responsible for supervising the current one.
pub type ParentRef = AgentHandle;
