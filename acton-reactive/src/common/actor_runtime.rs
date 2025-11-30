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

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

use acton_ern::Ern;
use dashmap::DashMap;
use futures::future::join_all;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::{error, trace}; // Added error import

use crate::actor::{ActorConfig, Idle, ManagedActor};
use crate::common::acton_inner::ActonInner;
use crate::common::{ActonApp, ActonConfig, ActorHandle, Broker, BrokerRef};
use crate::traits::ActorHandleInterface;

/// Represents the initialized and active Acton actor system runtime.
///
/// This struct is obtained after successfully launching the system via [`ActonApp::launch()`].
/// It holds the internal state of the running system, including a reference to the
/// central message broker and a registry of top-level actors.
///
/// `ActorRuntime` provides the primary methods for interacting with the system as a whole,
/// such as creating new top-level actors (`new_actor`, `spawn_actor`, etc.) and initiating
/// a graceful shutdown of all actors (`shutdown_all`).
///
/// It is cloneable, allowing different parts of an application to hold references
/// to the runtime environment.
#[derive(Debug, Clone, Default)]
pub struct ActorRuntime(pub(crate) ActonInner); // Keep inner field crate-public

impl ActorRuntime {
    /// Creates a new top-level actor builder (`ManagedActor<Idle, State>`) with a specified root name.
    ///
    /// This method initializes a [`ManagedActor`] in the [`Idle`] state, configured with a
    /// root [`Ern`] derived from the provided `name` and linked to the system's broker.
    /// The actor is registered as a top-level actor within the runtime.
    ///
    /// The returned actor is ready for further configuration (e.g., adding message handlers
    /// via `act_on`) before being started by calling `.start()` on it.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The user-defined state type for the actor. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `name`: A string that will form the root name of the actor's [`Ern`].
    ///
    /// # Returns
    ///
    /// A [`ManagedActor<Idle, State>`] instance, ready for configuration and starting.
    ///
    /// # Panics
    ///
    /// Panics if creating the root `Ern` from the provided `name` fails or if creating the internal `ActorConfig` fails.
    pub fn new_actor_with_name<State>(&mut self, name: String) -> ManagedActor<Idle, State>
    where
        State: Default + Send + Debug + 'static,
    {
        let actor_config = ActorConfig::new(
            Ern::with_root(name).expect("Failed to create root Ern for new actor"), // Use expect for clarity
            None,                        // No parent for top-level actor
            Some(self.0.broker.clone()), // Use system broker
        )
        .expect("Failed to create actor config");

        let runtime = self.clone();
        let new_actor = ManagedActor::new(Some(&runtime), Some(&actor_config));
        trace!("Registering new top-level actor: {}", new_actor.id());
        self.0
            .roots
            .insert(new_actor.id.clone(), new_actor.handle.clone());
        new_actor
    }

    /// Creates a new top-level actor builder (`ManagedActor<Idle, State>`) with a default name ("actor").
    ///
    /// Similar to [`ActorRuntime::new_actor_with_name`], but uses a default root name "actor"
    /// for the actor's [`Ern`]. The actor is registered as a top-level actor within the runtime.
    ///
    /// The returned actor is ready for further configuration before being started via `.start()`.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The user-defined state type for the actor. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Returns
    ///
    /// A [`ManagedActor<Idle, State>`] instance, ready for configuration and starting.
    ///
    /// # Panics
    ///
    /// Panics if creating the internal `ActorConfig` fails.
    pub fn new_actor<State>(&mut self) -> ManagedActor<Idle, State>
    where
        State: Default + Send + Debug + 'static,
    {
        // Use a default name if none is provided.
        self.new_actor_with_name("actor".to_string()) // Reuse the named version
    }

    /// Returns the number of top-level actors currently registered in the runtime.
    ///
    /// This count only includes actors directly created via the `ActorRuntime` and
    /// does not include child actors supervised by other actors.
    #[inline]
    #[must_use]
    pub fn actor_count(&self) -> usize {
        self.0.roots.len()
    }

    /// Creates a new top-level actor builder (`ManagedActor<Idle, State>`) using a provided configuration.
    ///
    /// This method initializes a [`ManagedActor`] in the [`Idle`] state using the specified
    /// [`ActorConfig`]. It ensures the actor is configured with the system's broker if not
    /// already set in the config. The actor is registered as a top-level actor within the runtime.
    ///
    /// The returned actor is ready for further configuration before being started via `.start()`.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The user-defined state type for the actor. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `config`: The [`ActorConfig`] to use for the new actor. The broker field will be
    ///   overridden with the system broker if it's `None`.
    ///
    /// # Returns
    ///
    /// A [`ManagedActor<Idle, State>`] instance, ready for configuration and starting.
    pub fn new_actor_with_config<State>(
        &mut self,
        mut config: ActorConfig,
    ) -> ManagedActor<Idle, State>
    where
        State: Default + Send + Debug + 'static,
    {
        let acton_ready = self.clone();
        // Ensure the actor uses the system broker if none is specified.
        if config.broker.is_none() {
            config.broker = Some(self.0.broker.clone());
        }
        let new_actor = ManagedActor::new(Some(&acton_ready), Some(&config));
        trace!(
            "Created new actor builder with config, id: {}",
            new_actor.id()
        );
        self.0
            .roots
            .insert(new_actor.id.clone(), new_actor.handle.clone());
        new_actor
    }

    /// Returns a clone of the handle ([`BrokerRef`]) to the system's central message broker.
    #[inline]
    #[must_use]
    pub fn broker(&self) -> BrokerRef {
        self.0.broker.clone()
    }

    /// Returns a clone of the Arc-wrapped IPC type registry.
    ///
    /// The registry is used to register message types for cross-process
    /// serialization and deserialization. Message types must be registered
    /// before they can be received via IPC.
    ///
    /// Only available when the `ipc` feature is enabled.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use acton_reactive::prelude::*;
    /// use serde::{Serialize, Deserialize};
    ///
    /// #[derive(Clone, Debug, Serialize, Deserialize)]
    /// struct PriceUpdate {
    ///     symbol: String,
    ///     price: f64,
    /// }
    ///
    /// let mut runtime = ActonApp::launch();
    ///
    /// // Register the message type with a stable name
    /// runtime.ipc_registry().register::<PriceUpdate>("PriceUpdate");
    /// ```
    #[cfg(feature = "ipc")]
    #[inline]
    #[must_use]
    pub fn ipc_registry(&self) -> std::sync::Arc<crate::common::ipc::IpcTypeRegistry> {
        self.0.ipc_type_registry.clone()
    }

    /// Exposes an actor for IPC access with a logical name.
    ///
    /// External processes reference actors by logical names (e.g., `price_service`)
    /// rather than full ERNs. This method registers the mapping between a
    /// human-readable name and the actor's handle.
    ///
    /// Only available when the `ipc` feature is enabled.
    ///
    /// # Arguments
    ///
    /// * `name`: The logical name to expose the actor as. External IPC clients
    ///   will use this name to target the actor.
    /// * `handle`: The [`ActorHandle`] of the actor to expose.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut runtime = ActonApp::launch();
    /// let actor = runtime.new_actor_with_name::<PriceServiceState>("price_service".to_string());
    /// let handle = actor.start().await;
    ///
    /// // Expose the actor for IPC access
    /// runtime.ipc_expose("price_service", handle.clone());
    /// ```
    #[cfg(feature = "ipc")]
    pub fn ipc_expose(&self, name: &str, handle: ActorHandle) {
        trace!("Exposing actor {} for IPC as '{}'", handle.id(), name);
        self.0.ipc_actor_registry.insert(name.to_string(), handle);
    }

    /// Removes an actor from IPC exposure.
    ///
    /// After calling this method, external processes will no longer be able
    /// to send messages to the actor using the specified name.
    ///
    /// Only available when the `ipc` feature is enabled.
    ///
    /// # Arguments
    ///
    /// * `name`: The logical name to remove from IPC exposure.
    ///
    /// # Returns
    ///
    /// The removed [`ActorHandle`] if the name was registered, or `None` if
    /// no actor was registered with that name.
    #[cfg(feature = "ipc")]
    pub fn ipc_hide(&self, name: &str) -> Option<ActorHandle> {
        trace!("Hiding actor '{}' from IPC", name);
        self.0.ipc_actor_registry.remove(name).map(|(_, h)| h)
    }

    /// Looks up an actor handle by its IPC logical name.
    ///
    /// This is used internally by the IPC listener to route messages to
    /// the correct actor.
    ///
    /// Only available when the `ipc` feature is enabled.
    ///
    /// # Arguments
    ///
    /// * `name`: The logical name to look up.
    ///
    /// # Returns
    ///
    /// A clone of the [`ActorHandle`] if found, or `None` if no actor
    /// is registered with that name.
    #[cfg(feature = "ipc")]
    #[must_use]
    pub fn ipc_lookup(&self, name: &str) -> Option<ActorHandle> {
        self.0.ipc_actor_registry.get(name).map(|r| r.clone())
    }

    /// Returns the number of actors currently exposed for IPC.
    ///
    /// Only available when the `ipc` feature is enabled.
    #[cfg(feature = "ipc")]
    #[inline]
    #[must_use]
    pub fn ipc_actor_count(&self) -> usize {
        self.0.ipc_actor_registry.len()
    }

    /// Starts the IPC listener with the default configuration.
    ///
    /// This method loads IPC configuration from XDG-compliant locations and
    /// starts a Unix Domain Socket listener that accepts connections from
    /// external processes and routes messages to registered actors.
    ///
    /// The listener runs in a background task and will be automatically stopped
    /// when the runtime's cancellation token is triggered (e.g., during shutdown).
    ///
    /// Only available when the `ipc` feature is enabled.
    ///
    /// # Returns
    ///
    /// An [`IpcListenerHandle`](crate::common::ipc::IpcListenerHandle) for
    /// managing the listener lifecycle and accessing statistics.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The socket directory cannot be created
    /// - Another listener is already running at the socket path
    /// - The socket cannot be bound
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut runtime = ActonApp::launch();
    ///
    /// // Register message types and expose actors first
    /// runtime.ipc_registry().register::<MyMessage>("MyMessage");
    /// runtime.ipc_expose("my_actor", actor_handle);
    ///
    /// // Start the IPC listener
    /// let listener = runtime.start_ipc_listener().await?;
    ///
    /// // Check listener statistics
    /// println!("Active connections: {}", listener.stats.connections_active());
    /// ```
    #[cfg(feature = "ipc")]
    pub async fn start_ipc_listener(
        &self,
    ) -> Result<crate::common::ipc::IpcListenerHandle, crate::common::ipc::IpcError> {
        let config = crate::common::ipc::IpcConfig::load();
        self.start_ipc_listener_with_config(config).await
    }

    /// Starts the IPC listener with a custom configuration.
    ///
    /// This method allows you to provide a custom IPC configuration instead
    /// of loading from the default XDG locations.
    ///
    /// Only available when the `ipc` feature is enabled.
    ///
    /// # Arguments
    ///
    /// * `config` - Custom IPC configuration.
    ///
    /// # Returns
    ///
    /// An [`IpcListenerHandle`](crate::common::ipc::IpcListenerHandle) for
    /// managing the listener lifecycle.
    ///
    /// # Errors
    ///
    /// Same as [`start_ipc_listener`](Self::start_ipc_listener).
    #[cfg(feature = "ipc")]
    pub async fn start_ipc_listener_with_config(
        &self,
        config: crate::common::ipc::IpcConfig,
    ) -> Result<crate::common::ipc::IpcListenerHandle, crate::common::ipc::IpcError> {
        trace!("Starting IPC listener with config: {:?}", config);
        let handle = crate::common::ipc::start_listener(
            config,
            self.0.ipc_type_registry.clone(),
            self.0.ipc_actor_registry.clone(),
            self.0.cancellation_token.clone(),
        )
        .await?;

        // Store the subscription manager reference so the broker can forward broadcasts to IPC clients
        {
            let mut guard = self.0.ipc_subscription_manager.write();
            *guard = Some(handle.subscription_manager().clone());
        }

        Ok(handle)
    }

    /// Creates, configures, and starts a top-level actor using a provided configuration and setup function.
    ///
    /// This method combines actor creation (using `config`), custom asynchronous setup (`setup_fn`),
    /// and starting the actor. The `setup_fn` receives the actor in the `Idle` state, performs
    /// necessary configurations (like adding message handlers), and must call `.start()` to
    /// transition the actor to the `Started` state, returning its `ActorHandle`.
    ///
    /// The actor is registered as a top-level actor within the runtime.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The state type of the actor. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `config`: The [`ActorConfig`] to use for creating the actor. The broker field will be
    ///   overridden with the system broker if it's `None`.
    /// * `setup_fn`: An asynchronous closure that takes the `ManagedActor<Idle, State>`, configures it,
    ///   calls `.start()`, and returns the resulting `ActorHandle`. The closure must be `Send + 'static`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `ActorHandle` of the successfully spawned actor, or an error if
    /// actor creation or the `setup_fn` fails.
    pub async fn spawn_actor_with_setup_fn<State>(
        &mut self,
        mut config: ActorConfig,
        setup_fn: impl FnOnce(
            ManagedActor<Idle, State>,
        ) -> Pin<Box<dyn Future<Output = ActorHandle> + Send + 'static>>,
    ) -> anyhow::Result<ActorHandle>
    where
        State: Default + Send + Debug + 'static,
    {
        let acton_ready = self.clone();
        if config.broker.is_none() {
            config.broker = Some(self.0.broker.clone());
        }

        let new_actor = ManagedActor::new(Some(&acton_ready), Some(&config));
        let actor_id = new_actor.id().clone(); // Get ID before moving
        trace!("Running setup function for actor: {}", actor_id);
        let handle = setup_fn(new_actor).await; // Setup function consumes the actor and returns handle
        trace!("Actor {} setup complete, registering handle.", actor_id);
        self.0.roots.insert(handle.id.clone(), handle.clone()); // Register the returned handle
        Ok(handle)
    }

    /// Initiates a graceful shutdown of the entire Acton system.
    ///
    /// This method attempts to stop all registered top-level actors (and consequently their
    /// descendant children through the `stop` propagation mechanism) by sending them a
    /// [`SystemSignal::Terminate`]. It waits for all top-level actor tasks to complete.
    /// Finally, it stops the central message broker actor.
    ///
    /// # Returns
    ///
    /// An `anyhow::Result<()>` indicating whether the shutdown process completed successfully.
    /// Errors during the stopping of individual actors or the broker will be propagated.
    pub async fn shutdown_all(&mut self) -> anyhow::Result<()> {
        use std::time::Duration;
        use tokio::time::timeout as tokio_timeout;

        // Phase 1: Concurrently signal all root actors to terminate gracefully.
        trace!("Sending Terminate signal to all root actors.");
        let stop_futures: Vec<_> = self
            .0
            .roots
            .iter()
            .map(|item| {
                let handle = item.value().clone();
                async move {
                    if let Err(e) = handle.stop().await {
                        error!("Error stopping actor {}: {:?}", handle.id(), e);
                    }
                }
            })
            .collect();

        let timeout_ms: u64 = self
            .0
            .config
            .system_shutdown_timeout()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX);

        trace!("Waiting for all actors to finish gracefully...");
        if tokio_timeout(Duration::from_millis(timeout_ms), join_all(stop_futures))
            .await
            .is_err()
        {
            error!("System-wide shutdown timeout expired after {} ms. Forcefully cancelling remaining tasks.", timeout_ms);
            self.0.cancellation_token.cancel(); // Forceful cancellation
        } else {
            trace!("All actors completed gracefully.");
        }

        trace!("Stopping the system broker...");
        // Stop the broker actor, using same system shutdown timeout.
        if let Ok(res) =
            tokio_timeout(Duration::from_millis(timeout_ms), self.0.broker.stop()).await
        {
            res?
        } else {
            error!(
                "Timeout waiting for broker to shut down after {} ms",
                timeout_ms
            );
            return Err(anyhow::anyhow!(
                "Timeout while waiting for system broker to shut down after {timeout_ms} ms"
            ));
        }
        trace!("System shutdown complete.");
        Ok(())
    }

    /// Creates, configures, and starts a top-level actor using a default configuration and a setup function.
    ///
    /// This is a convenience method similar to [`ActorRuntime::spawn_actor_with_setup_fn`], but it
    /// automatically creates a default `ActorConfig` (with a default name and the system broker).
    /// The provided `setup_fn` configures and starts the actor.
    ///
    /// The actor is registered as a top-level actor within the runtime.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The state type of the actor. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `setup_fn`: An asynchronous closure that takes the `ManagedActor<Idle, State>`, configures it,
    ///   calls `.start()`, and returns the resulting `ActorHandle`. The closure must be `Send + 'static`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `ActorHandle` of the successfully spawned actor, or an error if
    /// actor creation or the `setup_fn` fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the default `ActorConfig` cannot be created.
    pub async fn spawn_actor<State>(
        &mut self,
        setup_fn: impl FnOnce(
            ManagedActor<Idle, State>,
        ) -> Pin<Box<dyn Future<Output = ActorHandle> + Send + 'static>>,
    ) -> anyhow::Result<ActorHandle>
    where
        State: Default + Send + Debug + 'static,
    {
        // Create a default config, ensuring the system broker is included.
        let config = ActorConfig::new(Ern::default(), None, Some(self.broker()))?;
        // Reuse the more general spawn function.
        self.spawn_actor_with_setup_fn(config, setup_fn).await
    }
}

/// Converts an [`ActonApp`] marker into an initialized `ActorRuntime`.
///
/// This implementation defines the system bootstrap process triggered by [`ActonApp::launch()`].
/// It performs the following steps:
/// 1. Loads configuration from XDG-compliant locations using [`ActonConfig::load()`].
/// 2. Spawns a background Tokio task dedicated to initializing the [`Broker`].
/// 3. Uses a `oneshot` channel to receive the `ActorHandle` of the initialized broker
///    back from the background task.
/// 4. **Blocks the current thread** using `tokio::task::block_in_place` while waiting
///    for the broker initialization to complete. This ensures that `ActonApp::launch()`
///    does not return until the core system components (like the broker) are ready.
/// 5. Constructs the `ActorRuntime` using the received broker handle and loaded configuration.
///
/// **Warning**: The use of `block_in_place` means this conversion should typically
/// only happen once at the very start of the application within the main thread
/// or a dedicated initialization thread, before the main asynchronous workload begins.
/// Calling this from within an existing Tokio runtime task could lead to deadlocks
/// or performance issues.
impl From<ActonApp> for ActorRuntime {
    fn from(_acton: ActonApp) -> Self {
        trace!("Starting Acton system initialization (From<ActonApp>)");

        // Load configuration from XDG-compliant locations
        let config = ActonConfig::load();
        trace!("Configuration loaded: {:?}", config);

        let (sender, receiver) = oneshot::channel();

        // Create runtime with loaded configuration
        let mut runtime = Self(ActonInner {
            broker: ActorHandle::default(),
            roots: DashMap::default(),
            cancellation_token: CancellationToken::new(),
            config,
            #[cfg(feature = "ipc")]
            ipc_type_registry: std::sync::Arc::new(crate::common::ipc::IpcTypeRegistry::new()),
            #[cfg(feature = "ipc")]
            ipc_actor_registry: std::sync::Arc::new(DashMap::new()),
            #[cfg(feature = "ipc")]
            ipc_subscription_manager: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        });

        // Spawn broker initialization in a separate task
        let runtime_clone = runtime.clone();

        // Assert that the cancellation_token is present in the clone before broker initialization
        assert!(
            !runtime_clone.0.cancellation_token.is_cancelled(),
            "ActonInner cancellation_token must be present and active before Broker initialization"
        );

        tokio::spawn(async move {
            trace!("Broker initialization task started.");
            let broker = Broker::initialize(runtime_clone).await;
            trace!("Broker initialization task finished, sending handle.");
            let _ = sender.send(broker); // Send broker handle back
        });

        trace!("Blocking current thread to wait for broker initialization...");
        // Block until the broker handle is received
        let broker = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { receiver.await.expect("Broker initialization failed") })
        });
        trace!("Broker handle received, constructing ActorRuntime.");
        runtime.0.broker = broker;

        // Create the runtime with the initialized broker and configuration
        runtime
    }
}
