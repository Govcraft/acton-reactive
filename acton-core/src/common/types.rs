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

/// A type alias for a map of reactors, indexed by `TypeId`.
pub(crate) type ReactorMap<ActorEntity> = DashMap<TypeId, ReactorItem<ActorEntity>>;

/// An enum representing different types of reactors for handling signals, messages, and futures.
pub enum ReactorItem<ActorEntity: Default + Send + Debug + 'static> {
    // A signal reactor, which reacts to signals.
    // SignalReactor(Box<SignalHandler<ActorEntity>>),
    /// A future reactor, which reacts to futures.
    FutureReactor(Box<FutureHandler<ActorEntity>>),
}

/// A type alias for a future reactor function.
pub(crate) type FutureHandler<ManagedEntity> = dyn for<'a, 'b> Fn(&mut ManagedAgent<Started, ManagedEntity>, &'b mut Envelope) -> FutureBox
+ Send
+ Sync
+ 'static;

/// A type alias for a boxed future.
pub(crate) type FutureBox = Pin<Box<dyn Future<Output=()> + Sync + Send + 'static>>;

/// A type alias for the sending end of an agent's message channel (`mpsc::Sender`).
pub(crate) type AgentSender = Sender<Envelope>;

/// A type alias for a stop signal, represented by an atomic boolean.
pub(crate) type HaltSignal = AtomicBool;

// /// A type alias for a lifecycle reactor function.
// pub(crate) type LifecycleHandler<ActorEntity, ManagedEntity> =
// dyn Fn(&mut ManagedAgent<ActorEntity, ManagedEntity>) + Send;

/// A type alias for an asynchronous lifecycle reactor function.
pub(crate) type AsyncLifecycleHandler<ManagedEntity> =
Box<dyn Fn(&ManagedAgent<Started, ManagedEntity>) -> FutureBox + Send + Sync + 'static>;

/// A type alias for an [`AgentHandle`] that refers to the central message broker agent.
///
/// The broker is a special agent responsible for broadcasting messages (like a central
/// notice board) to agents that subscribe to specific topics.

pub type BrokerRef = AgentHandle;
/// A type alias for an [`AgentHandle`] that refers to the agent that created this one (its parent).
///
/// This is used for hierarchical agent structures where parent agents might supervise
/// their children.
pub type ParentRef = AgentHandle;
