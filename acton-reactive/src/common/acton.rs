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

use dashmap::DashMap;
use tokio_util::sync::CancellationToken;
use tracing::trace;

use crate::common::acton_inner::ActonInner;
use crate::common::{ActonConfig, ActorHandle, ActorRuntime, Broker};

/// Represents the entry point for initializing the Acton actor system.
///
/// This struct serves as a marker type to initiate the system bootstrap process.
/// The primary ways to use it are via:
/// - [`ActonApp::launch_async()`] - Preferred when in an async context
/// - [`ActonApp::launch()`] - For synchronous contexts (will panic if called from async)
///
/// Creating an `ActonApp` instance directly is typically not necessary; use the launch methods instead.
#[derive(Default, Debug, Clone)]
pub struct ActonApp;

impl ActonApp {
    /// Initializes the Acton actor system asynchronously.
    ///
    /// This is the preferred initialization method when called from within
    /// an async context (e.g., inside a `#[tokio::main]` function, a `#[tokio::test]`,
    /// or from an async task).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use acton_reactive::prelude::*;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mut runtime = ActonApp::launch_async().await;
    ///     // Use runtime...
    ///     runtime.shutdown_all().await.unwrap();
    /// }
    /// ```
    ///
    /// # Returns
    ///
    /// An initialized [`ActorRuntime`] with the broker running.
    pub async fn launch_async() -> ActorRuntime {
        trace!("Starting Acton system initialization (async)");

        let config = ActonConfig::load();
        trace!("Configuration loaded: {:?}", config);

        let mut runtime = ActorRuntime(ActonInner {
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

        // Initialize broker directly - no spawn/channel dance needed
        trace!("Initializing broker...");
        let broker = Broker::initialize(runtime.clone()).await;
        runtime.0.broker = broker;

        trace!("Acton system initialization complete (async)");
        runtime
    }

    /// Initializes the Acton actor system synchronously.
    ///
    /// This method creates a temporary Tokio runtime internally to perform
    /// initialization. Use this when initializing from a synchronous context
    /// (e.g., at the start of `main()` before entering async code).
    ///
    /// # Panics
    ///
    /// Panics if called from within an existing Tokio runtime. Use
    /// [`launch_async()`](Self::launch_async) instead when in an async context.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use acton_reactive::prelude::*;
    ///
    /// fn main() {
    ///     let runtime = ActonApp::launch();
    ///
    ///     // Enter async context with the runtime
    ///     tokio::runtime::Runtime::new()
    ///         .unwrap()
    ///         .block_on(async {
    ///             // Use runtime...
    ///         });
    /// }
    /// ```
    ///
    /// # Returns
    ///
    /// An initialized [`ActorRuntime`] with the broker running.
    #[must_use]
    pub fn launch() -> ActorRuntime {
        // Guard: prevent calling from async context
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "ActonApp::launch() was called from within a Tokio runtime. \
             Use ActonApp::launch_async().await instead when in an async context."
        );

        trace!("Starting Acton system initialization (sync)");

        // Create a runtime specifically for initialization
        let rt = tokio::runtime::Runtime::new()
            .expect("Failed to create Tokio runtime for Acton initialization");

        rt.block_on(Self::launch_async())
    }
}
