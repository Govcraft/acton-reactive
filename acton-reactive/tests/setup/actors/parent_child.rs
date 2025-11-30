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

/// Represents the state (model) for a Parent actor in supervision tests.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
pub struct Parent {
    /// Placeholder for potential child-related state (not typically an actor handle itself).
    pub child: Child,
}

/// Represents the state (model) for a Child actor in supervision tests.
// The `#[acton_actor]` macro derives `Default`, `Clone`, and implements `Debug`.
#[acton_actor]
pub struct Child;
