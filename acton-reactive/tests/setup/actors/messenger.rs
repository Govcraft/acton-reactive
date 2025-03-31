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

// Import the macro directly from the acton_macro crate
use acton_macro::acton_actor;

/// Represents a minimal agent state (model) for tests where the agent's
/// primary purpose is message handling or interaction, without needing internal state.
// The `#[acton_actor]` macro likely derives `Default`, `Clone`, `Debug` and potentially other traits
// needed for the struct to be used as agent state via `runtime.new_agent::<Messenger>()`.
#[acton_actor]
// #[derive(Default, Debug, Clone)] // Removed as #[acton_actor] likely provides these
pub struct Messenger;
