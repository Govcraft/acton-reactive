//! Provides the core components for defining and managing actors (agents) within the Acton framework.
//!
//! This module includes:
//! - [`ManagedAgent`]: The central struct representing an agent's runtime and state machine.
//! - [`AgentConfig`]: Configuration used to initialize an agent.
//! - State markers like [`Idle`] and [`Started`] used with [`ManagedAgent`] to represent its lifecycle state.

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

pub use agent_config::AgentConfig;
pub use managed_agent::Idle;
pub use managed_agent::ManagedAgent;
pub use managed_agent::started::Started;

mod managed_agent;

mod agent_config;
