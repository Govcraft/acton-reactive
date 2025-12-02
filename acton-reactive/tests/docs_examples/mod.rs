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

//! Documentation Examples Tests
//!
//! This module contains tests that verify the code examples from the documentation
//! compile and run correctly. Each test file corresponds to a documentation page.
//!
//! Note: Dead code warnings are suppressed because these tests demonstrate API patterns
//! where not all struct fields may be read - the focus is on testing compilation and behavior.

// Suppress dead_code warnings for documentation examples
#![allow(dead_code)]

pub mod quick_start_your_first_actor;
pub mod quick_start_sending_messages;
pub mod your_first_actor;
pub mod actors_and_state;
pub mod messages_and_handlers;
pub mod handler_types;
pub mod actor_lifecycle;
pub mod supervision;
pub mod pub_sub;
pub mod replies_and_context;
pub mod core_concepts_what_are_actors;
pub mod core_concepts_messages_and_handlers;
pub mod core_concepts_the_actor_system;
