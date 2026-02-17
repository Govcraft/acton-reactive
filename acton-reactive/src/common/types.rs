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

//! Defines common internal type aliases and supporting structures used within `acton-reactive`.
//!
//! This module centralizes type definitions for futures, handlers, channels, and other
//! implementation details to improve code readability and maintainability. It also
//! defines public type aliases for specific uses of [`ActorHandle`].

use std::any::TypeId;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;

use std::collections::HashMap;

use tokio::sync::mpsc::Sender;

use crate::actor::{ManagedActor, Started};
use crate::common::ActorHandle;
use crate::message::Envelope;
use crate::traits::ActonMessageReply;

/// Crate-internal: Map storing message handlers (`TypeId` -> `ReactorItem`).
///
/// Uses `HashMap` instead of `DashMap` because reactor maps are populated during
/// the `Idle` phase and then read-only for the lifetime of the actor's wake loop,
/// which runs on a single task. This eliminates unnecessary atomic synchronization.
pub type ReactorMap<ActorEntity> = HashMap<TypeId, ReactorItem<ActorEntity>>;

/// Crate-internal: Enum wrapping different kinds of message/event handlers.
/// All variants are future-based reactors with different mutability and fallibility characteristics.
#[allow(dead_code)] // Allow dead_code for potential future variants like SignalReactor
pub enum ReactorItem<ActorEntity: Default + Send + Debug + 'static> {
    // SignalReactor(Box<SignalHandler<ActorEntity>>),
    /// A handler for an infallible operation that processes a message and returns a future.
    Mutable(Box<FutureHandler<ActorEntity>>),
    /// A handler for a fallible operation that processes a message and returns a `Result` future.
    MutableFallible(Box<FutureHandlerResult<ActorEntity>>),
    /// A handler for an infallible read-only operation that processes a message and returns a future.
    ReadOnly(Box<FutureHandlerReadOnly<ActorEntity>>),
    /// A handler for a fallible read-only operation that processes a message and returns a `Result` future.
    ReadOnlyFallible(Box<FutureHandlerReadOnlyResult<ActorEntity>>),
}

/// Crate-internal: Type alias for the function signature of a message handler
/// that returns a future.
/// An infallible handler that returns `()`.
pub type FutureHandler<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a mut ManagedActor<Started, ManagedEntity>, // Takes mutable actor in Started state
        &'b mut Envelope, // Takes mutable envelope containing the message
    ) -> FutureBox // Returns a pinned, boxed future
    + Send
    + Sync
    + 'static;

/// A fallible handler that returns a `Result`.
pub type FutureHandlerResult<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a mut ManagedActor<Started, ManagedEntity>,
        &'b mut Envelope,
    ) -> FutureBoxResult
    + Send
    + Sync
    + 'static;

/// A read-only handler that processes a message immutably and returns a future.
pub type FutureHandlerReadOnly<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a ManagedActor<Started, ManagedEntity>, // Read-only reference
        &'b mut Envelope,
    ) -> FutureBoxReadOnly
    + Send
    + Sync
    + 'static;

/// A fallible read-only handler that processes a message immutably and returns a `Result` future.
pub type FutureHandlerReadOnlyResult<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a ManagedActor<Started, ManagedEntity>, // Read-only reference
        &'b mut Envelope,
    ) -> FutureBoxReadOnlyResult
    + Send
    + Sync
    + 'static;

/// New: Handler for error types.
pub type ErrorHandler<ManagedEntity> = dyn for<'a, 'b> Fn(
        &'a mut ManagedActor<Started, ManagedEntity>,
        &'b mut Envelope,
        &(dyn std::error::Error + 'static),
    ) -> FutureBox
    + Send
    + Sync
    + 'static;

/// Crate-internal: Type alias for a pinned, boxed, dynamically dispatched future
/// with `Output = ()` that is `Send`, `Sync`, and `'static`.
/// This is the required return type for asynchronous message handlers (`act_on`).
pub type FutureBox = Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>;

/// New: Box for read-only future-based handlers returning ().
pub type FutureBoxReadOnly = Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>;

/// New: Box for Future-based handlers returning Result.
pub type FutureBoxResult = Pin<
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
pub type FutureBoxReadOnlyResult = Pin<
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

/// Crate-internal: Type alias for the sender part of an actor's MPSC channel.
pub type ActorSender = Sender<Envelope>;

/// Crate-internal: Type alias for the atomic boolean used as a halt signal for actors.
pub type HaltSignal = AtomicBool;

/// Crate-internal: Type alias for the function signature of an asynchronous lifecycle hook.
/// Wrapped in `Option` to avoid allocation for actors that don't use lifecycle hooks.
pub type AsyncLifecycleHandler<ManagedEntity> =
    Option<Box<dyn Fn(&ManagedActor<Started, ManagedEntity>) -> FutureBox + Send + Sync + 'static>>;

// --- Public Type Aliases ---

/// A type alias representing a handle ([`ActorHandle`]) specifically for the system message broker.
///
/// This alias provides semantic clarity when a handle refers to the central [`Broker`].
pub type BrokerRef = ActorHandle;

/// A type alias representing a handle ([`ActorHandle`]) specifically for an actor's parent (supervisor).
///
/// This alias provides semantic clarity in hierarchical actor structures, indicating that
/// the handle refers to the actor responsible for supervising the current one.
pub type ParentRef = ActorHandle;
