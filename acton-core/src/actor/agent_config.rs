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


use acton_ern::{Ern, ErnParser};

use crate::common::{BrokerRef, ParentRef};
use crate::traits::AgentHandleInterface;

/// Configuration parameters required to initialize a new agent.
///
/// This struct encapsulates the essential settings for creating an agent instance,
/// including its unique identity, its relationship within the agent hierarchy (parent),
/// and its connection to the system message broker.
///
/// The agent's identity is represented by an [`Ern`](acton_ern::Ern), which supports
/// hierarchical naming. If a `parent` agent is specified during configuration, the
/// final `Ern` of the new agent will be derived by appending its base `id` to the
/// parent's `Ern`.
#[derive(Default, Debug, Clone)]
pub struct AgentConfig {
    /// The unique identifier (`Ern`) for the agent.
    /// If created under a parent, this will be the fully resolved hierarchical ID.
    id: Ern,
    /// Optional handle to the system message broker.
    pub(crate) broker: Option<BrokerRef>,
    /// Optional handle to the agent's parent (supervisor).
    parent: Option<ParentRef>,
}

impl AgentConfig {
    /// Creates a new `AgentConfig` instance, potentially deriving a hierarchical ID.
    ///
    /// This constructor configures a new agent. If a `parent` handle is provided,
    /// the agent's final `id` (`Ern`) is constructed by appending the provided `id`
    /// segment to the parent's `Ern`. If no `parent` is provided, the `id` is used directly.
    ///
    /// # Arguments
    ///
    /// * `id` - The base identifier (`Ern`) for the agent. If `parent` is `Some`, this
    ///   acts as the final segment appended to the parent's ID. If `parent` is `None`,
    ///   this becomes the agent's root ID.
    /// * `parent` - An optional [`ParentRef`] (handle) to the supervising agent.
    /// * `broker` - An optional [`BrokerRef`] (handle) to the system message broker.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing the configured `AgentConfig` instance.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing the parent's ID string into an `Ern` fails when
    /// constructing a hierarchical ID.
    pub fn new(
        id: Ern,
        parent: Option<ParentRef>,
        broker: Option<BrokerRef>,
    ) -> anyhow::Result<AgentConfig> {
        if let Some(parent_ref) = parent { // Use a different variable name to avoid shadowing
            // Get the parent ERN
            let parent_id = ErnParser::new(parent_ref.id().to_string()).parse()?;
            let child_id = parent_id + id;
            Ok(AgentConfig {
                id: child_id,
                broker,
                parent: Some(parent_ref),
            })
        } else {
            Ok(AgentConfig {
                id,
                broker,
                parent, // parent is None here
            })
        }
    }

    /// Creates a new `AgentConfig` for a top-level agent with a root identifier.
    ///
    /// This is a convenience function for creating an `AgentConfig` for an agent
    /// that has no parent (i.e., it's a root agent in the hierarchy). The provided
    /// `name` is used to create a root [`Ern`](acton_ern::Ern).
    ///
    /// # Arguments
    ///
    /// * `name` - A string-like value that will be used as the root name for the agent's `Ern`.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing the new `AgentConfig` instance with no parent or broker.
    ///
    /// # Errors
    ///
    /// Returns an error if creating the root `Ern` from the provided `name` fails
    /// (e.g., if the name is invalid according to `Ern` rules).
    pub fn new_with_name(
        name: impl Into<String>,
    ) -> anyhow::Result<AgentConfig> {
        Self::new(Ern::with_root(name.into())?, None, None)
    }


    /// Returns a clone of the agent's unique identifier (`Ern`).
    #[inline]
    pub(crate) fn id(&self) -> Ern {
        self.id.clone()
    }

    /// Returns a reference to the optional broker handle.
    #[inline]
    pub(crate) fn get_broker(&self) -> &Option<BrokerRef> {
        &self.broker
    }

    /// Returns a reference to the optional parent handle.
    #[inline]
    pub(crate) fn parent(&self) -> &Option<ParentRef> {
        &self.parent
    }
}
