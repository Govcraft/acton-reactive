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

use std::any::Any;
use std::fmt::Debug;

/// A trait for message handler return values that can be downcast.
///
/// This trait is used to enable type-safe handling of the `Ok` variant
/// from a fallible message handler (`try_act_on`). By requiring `Any`,
/// we can downcast the trait object back to its concrete type.
pub trait ActonMessageReply: Any + Debug + Send {
    /// Returns self as `Any` so that it can be downcast.
    fn as_any(&self) -> &dyn Any;
}

impl<T: Any + Debug + Send> ActonMessageReply for T {
    fn as_any(&self) -> &dyn Any {
        self
    }
}
