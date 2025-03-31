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

// A simple actor for testing purposes
// tracks the overall number of jokes told

// Import the macro directly from the acton_macro crate
use acton_macro::acton_actor;

/// Represents the state (model) for a Comedian agent in tests.
///
/// This agent state typically tracks joke delivery and audience reactions.
// The `#[acton_actor]` macro likely derives `Default`, `Clone`, `Debug` and potentially other traits
// needed for the struct to be used as agent state via `runtime.new_agent::<Comedian>()`.
#[acton_actor]
// #[derive(Default, Debug, Clone)] // Removed as #[acton_actor] likely provides these
pub struct Comedian {
    /// Total number of jokes attempted by the comedian.
    pub jokes_told: usize,
    /// Number of jokes that received a positive reaction (e.g., Chuckle).
    pub funny: usize,
    /// Number of jokes that received a negative reaction (e.g., Groan).
    pub bombers: usize,
}
