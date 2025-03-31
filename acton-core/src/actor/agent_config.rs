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
use crate::traits::Actor;

/// Configuration for creating an agent.
///
/// This struct holds the necessary information to configure an agent,
/// including its unique identifier (`id`), broker, and parent reference.
/// The `id` uses the `Ern` type to support hierarchical naming if needed.
#[derive(Default, Debug, Clone)]
pub struct AgentConfig {
    /// The unique identifier for the agent. Can be hierarchical.
    id: Ern,
    pub(crate) broker: Option<BrokerRef>,
    parent: Option<ParentRef>,
}

impl AgentConfig {
    /// Creates a new `ActorConfig` instance.
    ///
    /// # Arguments
    ///
    /// * `id` - The base identifier (`Ern`) for the agent. If a `parent` is provided,
    ///   this `id` will be appended to the parent's ID to form the final hierarchical ID.
    /// * `parent` - An optional handle to the parent agent.
    /// * `broker` - An optional handle to a broker agent.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing the new `AgentConfig` instance or an error.
    pub fn new(
        id: Ern,
        parent: Option<ParentRef>,
        broker: Option<BrokerRef>,
    ) -> anyhow::Result<AgentConfig> {
        if let Some(parent) = parent {
            // Get the parent ERN
            let parent_id = ErnParser::new(parent.id().to_string()).parse()?;
            let child_id = parent_id + id;
            Ok(AgentConfig {
                id: child_id,
                broker,
                parent: Some(parent),
            })
        } else {
            Ok(AgentConfig {
                id,
                broker,
                parent,
            })
        }
    }

    /// Creates a new config with a root identifier (`Ern`) using the provided name.
    pub fn new_with_name(
        name: impl Into<String>,
    ) -> anyhow::Result<AgentConfig> {
        Self::new(Ern::with_root(name.into())?, None, None)
    }


    /// Returns the unique identifier (`Ern`) of the agent.
    pub(crate) fn id(&self) -> Ern {
        self.id.clone()
    }

    /// Returns a reference to the optional broker.
    pub(crate) fn get_broker(&self) -> &Option<BrokerRef> {
        &self.broker
    }

    /// Returns a reference to the optional parent.
    pub(crate) fn parent(&self) -> &Option<ParentRef> {
        &self.parent
    }
}
