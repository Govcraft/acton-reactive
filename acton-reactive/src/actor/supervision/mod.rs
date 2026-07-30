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

//! Supervision of child actors.
//!
//! Every type in this module is re-exported from here, so the public paths
//! [`SupervisionStrategy`] and [`SupervisionDecision`] resolve exactly as they
//! did when this module was a single file.
//!
//! See [`strategy`] for the supervision strategies themselves.

pub use strategy::{SupervisionDecision, SupervisionStrategy};

/// Contains the supervision strategies and the decisions they produce.
mod strategy;
