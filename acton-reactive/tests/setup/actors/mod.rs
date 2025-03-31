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

/// This module defines various simple structs used as agent state (models)
/// within the integration tests. These structs represent the internal data
/// held by different test agents like `Comedian`, `Counter`, etc.

pub use audience_member::AudienceMember;
pub use comedian::Comedian;
pub use counter::Counter;
pub use messenger::Messenger;
pub use pool_item::PoolItem;
pub use parent_child::Parent;

mod audience_member;
mod comedian;
mod counter;
mod messenger;
mod parent_child;
mod pool_item;
