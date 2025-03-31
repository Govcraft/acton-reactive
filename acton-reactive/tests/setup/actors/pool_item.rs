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
use tracing::error;

use acton_core::prelude::*;
use acton_macro::acton_actor;

use crate::setup::*;

/// Represents the state (model) for a generic "Pool Item" agent in tests.
///
/// This is often used in scenarios involving multiple child agents managed by a parent,
/// or simply as a basic agent that needs to count received messages.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
pub struct PoolItem {
    /// Tracks the number of messages (typically `Ping`) received by this agent.
    pub receive_count: usize, // Tracks the number of received events
}
