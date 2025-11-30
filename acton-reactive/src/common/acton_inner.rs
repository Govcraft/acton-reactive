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

use acton_ern::Ern;
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

use crate::common::{ActonConfig, ActorHandle, BrokerRef};

#[cfg(feature = "ipc")]
use crate::common::ipc::{IpcTypeRegistry, SubscriptionManager};
#[cfg(feature = "ipc")]
use parking_lot::RwLock;
#[cfg(feature = "ipc")]
use std::sync::Arc;

/// Internal state structure for the Acton runtime.
///
/// This struct holds all the core components needed to manage the actor system,
/// including the message broker, actor registry, and configuration.
#[derive(Debug, Clone)]
pub struct ActonInner {
    /// Handle to the central message broker actor.
    pub(crate) broker: BrokerRef,

    /// Registry of top-level (root) actors, keyed by their ERN.
    pub(crate) roots: DashMap<Ern, ActorHandle>,

    /// Token for coordinating graceful shutdown across all actors.
    pub(crate) cancellation_token: CancellationToken,

    /// Runtime configuration loaded from XDG-compliant locations.
    pub(crate) config: ActonConfig,

    /// Registry for IPC message type deserialization.
    ///
    /// Only available when the `ipc` feature is enabled.
    #[cfg(feature = "ipc")]
    pub(crate) ipc_type_registry: Arc<IpcTypeRegistry>,

    /// Registry mapping logical names to actor handles for IPC routing.
    ///
    /// External processes reference actors by logical names (e.g., `price_service`)
    /// rather than full ERNs. This registry maintains that mapping.
    ///
    /// Only available when the `ipc` feature is enabled.
    #[cfg(feature = "ipc")]
    pub(crate) ipc_actor_registry: Arc<DashMap<String, ActorHandle>>,

    /// Subscription manager for IPC push notifications.
    ///
    /// This is set when an IPC listener is started and enables the broker
    /// to forward broadcasts to external IPC clients that have subscribed
    /// to specific message types.
    ///
    /// Only available when the `ipc` feature is enabled.
    #[cfg(feature = "ipc")]
    pub(crate) ipc_subscription_manager: Arc<RwLock<Option<Arc<SubscriptionManager>>>>,
}

impl Default for ActonInner {
    fn default() -> Self {
        Self {
            broker: ActorHandle::default(),
            roots: DashMap::default(),
            cancellation_token: CancellationToken::new(),
            config: ActonConfig::default(),
            #[cfg(feature = "ipc")]
            ipc_type_registry: Arc::new(IpcTypeRegistry::new()),
            #[cfg(feature = "ipc")]
            ipc_actor_registry: Arc::new(DashMap::new()),
            #[cfg(feature = "ipc")]
            ipc_subscription_manager: Arc::new(RwLock::new(None)),
        }
    }
}
