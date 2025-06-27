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
use futures::future::join_all;
use tokio::sync::oneshot;
use tracing::{error, trace}; // Added error import

use crate::actor::{AgentConfig, Idle, ManagedAgent};
use crate::common::acton_inner::ActonInner;
use crate::common::{ActonApp, AgentBroker, AgentHandle, BrokerRef};
use crate::traits::AgentHandleInterface;

/// Represents the initialized and active Acton agent system runtime.
///
/// This struct is obtained after successfully launching the system via [`ActonApp::launch()`].
/// It holds the internal state of the running system, including a reference to the
/// central message broker and a registry of top-level agents.
///
/// `AgentRuntime` provides the primary methods for interacting with the system as a whole,
/// such as creating new top-level agents (`new_agent`, `spawn_agent`, etc.) and initiating
/// a graceful shutdown of all agents (`shutdown_all`).
///
/// It is cloneable, allowing different parts of an application to hold references
/// to the runtime environment.
#[derive(Debug, Clone, Default)]
pub struct AgentRuntime(pub(crate) ActonInner); // Keep inner field crate-public

impl AgentRuntime {
    /// Creates a new top-level agent builder (`ManagedAgent<Idle, State>`) with a specified root name.
    ///
    /// This method initializes a [`ManagedAgent`] in the [`Idle`] state, configured with a
    /// root [`Ern`] derived from the provided `name` and linked to the system's broker.
    /// The agent is registered as a top-level agent within the runtime.
    ///
    /// The returned agent is ready for further configuration (e.g., adding message handlers
    /// via `act_on`) before being started by calling `.start()` on it.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The user-defined state type for the agent. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `name`: A string that will form the root name of the agent's [`Ern`].
    ///
    /// # Returns
    ///
    /// A [`ManagedAgent<Idle, State>`] instance, ready for configuration and starting.
    ///
    /// # Panics
    ///
    /// Panics if creating the root `Ern` from the provided `name` fails or if creating the internal `AgentConfig` fails.
    pub async fn new_agent_with_name<State>(&mut self, name: String) -> ManagedAgent<Idle, State>
    where
        State: Default + Send + Debug + 'static,
    {
        let actor_config = AgentConfig::new(
            Ern::with_root(name).expect("Failed to create root Ern for new agent"), // Use expect for clarity
            None,                        // No parent for top-level agent
            Some(self.0.broker.clone()), // Use system broker
        )
        .expect("Failed to create actor config");

        let runtime = self.clone();
        let new_actor = ManagedAgent::new(&Some(runtime), Some(actor_config)).await;
        trace!("Registering new top-level agent: {}", new_actor.id());
        self.0
            .roots
            .insert(new_actor.id.clone(), new_actor.handle.clone());
        new_actor
    }

    /// Creates a new top-level agent builder (`ManagedAgent<Idle, State>`) with a default name ("agent").
    ///
    /// Similar to [`AgentRuntime::new_agent_with_name`], but uses a default root name "agent"
    /// for the agent's [`Ern`]. The agent is registered as a top-level agent within the runtime.
    ///
    /// The returned agent is ready for further configuration before being started via `.start()`.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The user-defined state type for the agent. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Returns
    ///
    /// A [`ManagedAgent<Idle, State>`] instance, ready for configuration and starting.
    ///
    /// # Panics
    ///
    /// Panics if creating the internal `AgentConfig` fails.
    pub async fn new_agent<State>(&mut self) -> ManagedAgent<Idle, State>
    where
        State: Default + Send + Debug + 'static,
    {
        // Use a default name if none is provided.
        self.new_agent_with_name("agent".to_string()).await // Reuse the named version
    }

    /// Returns the number of top-level agents currently registered in the runtime.
    ///
    /// This count only includes agents directly created via the `AgentRuntime` and
    /// does not include child agents supervised by other agents.
    #[inline]
    pub fn agent_count(&self) -> usize {
        self.0.roots.len()
    }

    /// Creates a new top-level agent builder (`ManagedAgent<Idle, State>`) using a provided configuration.
    ///
    /// This method initializes a [`ManagedAgent`] in the [`Idle`] state using the specified
    /// [`AgentConfig`]. It ensures the agent is configured with the system's broker if not
    /// already set in the config. The agent is registered as a top-level agent within the runtime.
    ///
    /// The returned agent is ready for further configuration before being started via `.start()`.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The user-defined state type for the agent. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `config`: The [`AgentConfig`] to use for the new agent. The broker field will be
    ///   overridden with the system broker if it's `None`.
    ///
    /// # Returns
    ///
    /// A [`ManagedAgent<Idle, State>`] instance, ready for configuration and starting.
    pub async fn new_agent_with_config<State>(
        &mut self,
        mut config: AgentConfig,
    ) -> ManagedAgent<Idle, State>
    where
        State: Default + Send + Debug + 'static,
    {
        let acton_ready = self.clone();
        // Ensure the agent uses the system broker if none is specified.
        if config.broker.is_none() {
            config.broker = Some(self.0.broker.clone());
        }
        let new_agent = ManagedAgent::new(&Some(acton_ready), Some(config)).await;
        trace!(
            "Created new agent builder with config, id: {}",
            new_agent.id()
        );
        self.0
            .roots
            .insert(new_agent.id.clone(), new_agent.handle.clone());
        new_agent
    }

    /// Returns a clone of the handle ([`BrokerRef`]) to the system's central message broker.
    #[inline]
    pub fn broker(&self) -> BrokerRef {
        self.0.broker.clone()
    }

    /// Creates, configures, and starts a top-level agent using a provided configuration and setup function.
    ///
    /// This method combines agent creation (using `config`), custom asynchronous setup (`setup_fn`),
    /// and starting the agent. The `setup_fn` receives the agent in the `Idle` state, performs
    /// necessary configurations (like adding message handlers), and must call `.start()` to
    /// transition the agent to the `Started` state, returning its `AgentHandle`.
    ///
    /// The agent is registered as a top-level agent within the runtime.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The state type of the agent. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `config`: The [`AgentConfig`] to use for creating the agent. The broker field will be
    ///   overridden with the system broker if it's `None`.
    /// * `setup_fn`: An asynchronous closure that takes the `ManagedAgent<Idle, State>`, configures it,
    ///   calls `.start()`, and returns the resulting `AgentHandle`. The closure must be `Send + 'static`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `AgentHandle` of the successfully spawned agent, or an error if
    /// agent creation or the `setup_fn` fails.
    pub async fn spawn_agent_with_setup_fn<State>(
        &mut self,
        mut config: AgentConfig,
        setup_fn: impl FnOnce(
            ManagedAgent<Idle, State>,
        ) -> Pin<Box<dyn Future<Output = AgentHandle> + Send + 'static>>,
    ) -> anyhow::Result<AgentHandle>
    where
        State: Default + Send + Debug + 'static,
    {
        let acton_ready = self.clone();
        if config.broker.is_none() {
            config.broker = Some(self.0.broker.clone());
        }

        let new_agent = ManagedAgent::new(&Some(acton_ready), Some(config)).await;
        let agent_id = new_agent.id().clone(); // Get ID before moving
        trace!("Running setup function for agent: {}", agent_id);
        let handle = setup_fn(new_agent).await; // Setup function consumes the agent and returns handle
        trace!("Agent {} setup complete, registering handle.", agent_id);
        self.0.roots.insert(handle.id.clone(), handle.clone()); // Register the returned handle
        Ok(handle)
    }

    /// Initiates a graceful shutdown of the entire Acton system.
    ///
    /// This method attempts to stop all registered top-level agents (and consequently their
    /// descendant children through the `stop` propagation mechanism) by sending them a
    /// [`SystemSignal::Terminate`]. It waits for all top-level agent tasks to complete.
    /// Finally, it stops the central message broker agent.
    ///
    /// # Returns
    ///
    /// An `anyhow::Result<()>` indicating whether the shutdown process completed successfully.
    /// Errors during the stopping of individual agents or the broker will be propagated.
    pub async fn shutdown_all(&mut self) -> anyhow::Result<()> {
        use std::env;
        use std::time::Duration;
        use tokio::time::timeout as tokio_timeout;

        trace!("Initiating shutdown of all top-level agents...");
        // Collect stop futures for all root agents.
        let stop_futures = self.0.roots.iter().map(|item| {
            let root_handle = item.value().clone();
            async move {
                trace!("Sending stop signal to root agent: {}", root_handle.id());
                root_handle.stop().await // Call stop on the handle
            }
        });

        // Determine system shutdown timeout from env or use default (30s)
        let timeout_ms: u64 = env::var("ACTON_SYSTEM_SHUTDOWN_TIMEOUT_MS")
            .ok()
            .and_then(|val| val.parse().ok())
            .unwrap_or(30_000);

        // Wait for all root agents (and their children) to stop, with timeout.
        let roots_fut = join_all(stop_futures);
        let results: Vec<anyhow::Result<()>> = match tokio_timeout(
            Duration::from_millis(timeout_ms),
            roots_fut,
        )
        .await
        {
            Ok(r) => r,
            Err(_) => {
                error!("System-wide shutdown timeout expired after {} ms. Some agents may not have stopped.", timeout_ms);
                return Err(anyhow::anyhow!(
                    "Timeout: not all root agents stopped within {} ms",
                    timeout_ms
                ));
            }
        };
        trace!("All root agent stop futures completed.");

        // Check for errors during agent shutdown.
        for result in results {
            if let Err(e) = result {
                // Log error but continue shutdown attempt
                error!("Error stopping agent during shutdown: {:?}", e);
            }
        }

        trace!("Stopping the system broker...");
        // Stop the broker agent, using same system shutdown timeout.
        match tokio_timeout(Duration::from_millis(timeout_ms), self.0.broker.stop()).await {
            Ok(res) => res?,
            Err(_) => {
                error!(
                    "Timeout waiting for broker to shut down after {} ms",
                    timeout_ms
                );
                return Err(anyhow::anyhow!(
                    "Timeout while waiting for system broker to shut down after {} ms",
                    timeout_ms
                ));
            }
        }
        trace!("System shutdown complete.");
        Ok(())
    }

    /// Creates, configures, and starts a top-level agent using a default configuration and a setup function.
    ///
    /// This is a convenience method similar to [`AgentRuntime::spawn_agent_with_setup_fn`], but it
    /// automatically creates a default `AgentConfig` (with a default name and the system broker).
    /// The provided `setup_fn` configures and starts the agent.
    ///
    /// The agent is registered as a top-level agent within the runtime.
    ///
    /// # Type Parameters
    ///
    /// * `State`: The state type of the agent. Must implement `Default`, `Send`, `Debug`, and be `'static`.
    ///
    /// # Arguments
    ///
    /// * `setup_fn`: An asynchronous closure that takes the `ManagedAgent<Idle, State>`, configures it,
    ///   calls `.start()`, and returns the resulting `AgentHandle`. The closure must be `Send + 'static`.
    ///
    /// # Returns
    ///
    /// A `Result` containing the `AgentHandle` of the successfully spawned agent, or an error if
    /// agent creation or the `setup_fn` fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the default `AgentConfig` cannot be created.
    pub async fn spawn_agent<State>(
        &mut self,
        setup_fn: impl FnOnce(
            ManagedAgent<Idle, State>,
        ) -> Pin<Box<dyn Future<Output = AgentHandle> + Send + 'static>>,
    ) -> anyhow::Result<AgentHandle>
    where
        State: Default + Send + Debug + 'static,
    {
        // Create a default config, ensuring the system broker is included.
        let config = AgentConfig::new(Ern::default(), None, Some(self.broker()))?;
        // Reuse the more general spawn function.
        self.spawn_agent_with_setup_fn(config, setup_fn).await
    }
}

/// Converts an [`ActonApp`] marker into an initialized `AgentRuntime`.
///
/// This implementation defines the system bootstrap process triggered by [`ActonApp::launch()`].
/// It performs the following steps:
/// 1. Spawns a background Tokio task dedicated to initializing the [`AgentBroker`].
/// 2. Uses a `oneshot` channel to receive the `AgentHandle` of the initialized broker
///    back from the background task.
/// 3. **Blocks the current thread** using `tokio::task::block_in_place` while waiting
///    for the broker initialization to complete. This ensures that `ActonApp::launch()`
///    does not return until the core system components (like the broker) are ready.
/// 4. Constructs the `AgentRuntime` using the received broker handle.
///
/// **Warning**: The use of `block_in_place` means this conversion should typically
/// only happen once at the very start of the application within the main thread
/// or a dedicated initialization thread, before the main asynchronous workload begins.
/// Calling this from within an existing Tokio runtime task could lead to deadlocks
/// or performance issues.
impl From<ActonApp> for AgentRuntime {
    fn from(_acton: ActonApp) -> Self {
        trace!("Starting Acton system initialization (From<ActonApp>)");
        let (sender, receiver) = oneshot::channel();

        // Spawn broker initialization in a separate task.
        tokio::spawn(async move {
            trace!("Broker initialization task started.");
            let broker = AgentBroker::initialize().await;
            trace!("Broker initialization task finished, sending handle.");
            let _ = sender.send(broker); // Send broker handle back
        });

        trace!("Blocking current thread to wait for broker initialization...");
        // Block until the broker handle is received.
        let broker = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async { receiver.await.expect("Broker initialization failed") })
        });
        trace!("Broker handle received, constructing AgentRuntime.");

        // Create the runtime with the initialized broker.
        AgentRuntime(ActonInner {
            broker,
            ..Default::default()
        })
    }
}
