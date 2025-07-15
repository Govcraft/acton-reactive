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
use crate::traits::ActonMessageReply;

/// Crate-internal: Map storing message handlers (`TypeId` -> `ReactorItem`).
pub(crate) type ReactorMap<ActorEntity> = DashMap<TypeId, ReactorItem<ActorEntity>>;

/// Crate-internal: Enum wrapping different kinds of message/event handlers.
/// Currently only supports `FutureReactor`.
#[allow(dead_code)] // Allow dead_code for potential future variants like SignalReactor
pub(crate) enum ReactorItem<ActorEntity: Default + Send + Debug + 'static> {
    // SignalReactor(Box<SignalHandler<ActorEntity>>),
    /// A handler for an infallible operation that processes a message and returns a future.
    FutureReactor(Box<FutureHandler<ActorEntity>>),
    /// A handler for a fallible operation that processes a message and returns a `Result` future.
    FutureReactorResult(Box<FutureHandlerResult<ActorEntity>>),
    /// A handler for an infallible read-only operation that processes a message and returns a future.
    FutureReactorReadOnly(Box<FutureHandlerReadOnly<ActorEntity>>),
    /// A handler for a fallible read-only operation that processes a message and returns a `Result` future.
    FutureReactorReadOnlyResult(Box<FutureHandlerReadOnlyResult<ActorEntity>>),
}

/// Crate-internal: Type alias for the function signature of a message handler
/// that returns a future.
/// An infallible handler that returns `()`.
pub(crate) type FutureHandler<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a mut ManagedAgent<Started, ManagedEntity>, // Takes mutable agent in Started state
        &'b mut Envelope, // Takes mutable envelope containing the message
    ) -> FutureBox // Returns a pinned, boxed future
    + Send
    + Sync
    + 'static;

/// A fallible handler that returns a `Result`.
pub(crate) type FutureHandlerResult<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a mut ManagedAgent<Started, ManagedEntity>,
        &'b mut Envelope,
    ) -> FutureBoxResult
    + Send
    + Sync
    + 'static;

/// A read-only handler that processes a message immutably and returns a future.
pub(crate) type FutureHandlerReadOnly<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a ManagedAgent<Started, ManagedEntity>,  // Read-only reference
        &'b mut Envelope,
    ) -> FutureBoxReadOnly
    + Send
    + Sync
    + 'static;

/// A fallible read-only handler that processes a message immutably and returns a `Result` future.
pub(crate) type FutureHandlerReadOnlyResult<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a ManagedAgent<Started, ManagedEntity>,  // Read-only reference
        &'b mut Envelope,
    ) -> FutureBoxReadOnlyResult
    + Send
    + Sync
    + 'static;

/// New: Handler for error types.
pub(crate) type ErrorHandler<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a mut ManagedAgent<Started, ManagedEntity>,
        &'b mut Envelope,
        &(dyn std::error::Error + 'static),
    ) -> FutureBox
    + Send
    + Sync
    + 'static;

/// Crate-internal: Type alias for a pinned, boxed, dynamically dispatched future
/// with `Output = ()` that is `Send`, `Sync`, and `'static`.
/// This is the required return type for asynchronous message handlers (`act_on`).
pub(crate) type FutureBox = Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>;

/// New: Box for read-only future-based handlers returning ().
pub(crate) type FutureBoxReadOnly = Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>;

/// New: Box for Future-based handlers returning Result.
pub(crate) type FutureBoxResult = Pin<
    Box<
        dyn Future<
                Output = Result<
                    Box<dyn ActonMessageReply + Send>,
                    (Box<dyn std::error::Error + Send + Sync>, TypeId),
                >,
            > + Send
            + Sync
            + 'static,
    >,
>;

/// New: Box for read-only Future-based handlers returning Result.
pub(crate) type FutureBoxReadOnlyResult = Pin<
    Box<
        dyn Future<
                Output = Result<
                    Box<dyn ActonMessageReply + Send>,
                    (Box<dyn std::error::Error + Send + Sync>, TypeId),
                >,
            > + Send
            + Sync
            + 'static,
    >,
>;

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
