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

use crate::common::AgentRuntime;

/// Represents the entry point for initializing the Acton agent system.
///
/// This struct serves as a marker type to initiate the system bootstrap process.
/// The primary way to use it is via the associated function [`ActonApp::launch`],
/// which sets up the core components like the message broker and returns an
/// [`AgentRuntime`] instance representing the active system.
///
/// Creating an `ActonApp` instance directly is typically not necessary; use [`ActonApp::launch`] instead.
#[derive(Default, Debug, Clone)]
pub struct ActonApp;

impl ActonApp {
    /// Initializes and launches the Acton agent system.
    ///
    /// This is the main entry point for starting the Acton framework. It performs
    /// the necessary setup, including initializing the system message broker
    /// and creating the core runtime environment.
    ///
    /// The process involves:
    /// 1. Creating a default `ActonApp` instance.
    /// 2. Converting this instance into an [`AgentRuntime`] via the `From<ActonApp>` trait.
    ///    This conversion triggers the actual initialization logic (e.g., broker setup)
    ///    defined within the `From` implementation for `AgentRuntime`.
    ///
    /// # Returns
    ///
    /// An [`AgentRuntime`] instance representing the successfully launched and operational
    /// Acton system. This runtime can then be used to spawn top-level agents.
    pub fn launch() -> AgentRuntime {
        let system: ActonApp = Default::default();
        // The .into() call triggers the From<ActonApp> for AgentRuntime,
        // which performs the actual system initialization.
        system.into()
    }
}
