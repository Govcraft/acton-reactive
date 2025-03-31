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

// We'll create pool of audience member actor who will hear a joke told by the comedian
// They will randomly react to the jokes after which the Comedian will report on how many
// jokes landed and didn't land

use rand::Rng;
use tracing::{debug, error, info, trace};

use acton_macro::acton_actor;

use crate::setup::*;

/// Represents the state (model) for an Audience Member agent in tests.
/// This agent typically reacts randomly to `Joke` messages.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
pub struct AudienceMember {
    /// Counter for how many jokes this audience member has heard.
    pub jokes_told: usize,
    /// Counter for how many jokes were found funny (e.g., resulted in Chuckle).
    pub funny: usize,
    /// Counter for how many jokes bombed (e.g., resulted in Groan).
    pub bombers: usize,
}
