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

use dyn_clone::DynClone; // Required for cloning trait objects

/// A marker trait for types that can be used as messages within the Acton framework.
///
/// This trait combines several standard library traits (`Any`, `Send`, `Sync`, `Debug`)
/// with [`DynClone`] to ensure that messages are safe to send between threads,
/// can be dynamically cloned (even as trait objects), support downcasting back to
/// their concrete types, and are debuggable.
///
/// The `as_any` and `as_any_mut` methods are crucial for the framework's ability
/// to handle messages generically and perform type-based dispatch (e.g., in message
/// handlers registered via [`ManagedActor::act_on`](crate::actor::ManagedActor::act_on)).
///
/// A blanket implementation is provided, so any type `T` that satisfies the bounds
/// (`T: Any + Send + Sync + Debug + DynClone + 'static`) automatically implements
/// `ActonMessage`. Users typically only need to ensure their message structs/enums
/// derive `Clone` and `Debug` and meet the `Send + Sync + 'static` requirements.
pub trait ActonMessage: DynClone + Any + Send + Sync + Debug {
    /// Returns a reference to the message as a dynamic [`Any`] trait object.
    ///
    /// This allows for runtime type introspection and downcasting using methods like
    /// [`Any::downcast_ref`](std::any::Any::downcast_ref).
    fn as_any(&self) -> &dyn Any;

    /// Returns a mutable reference to the message as a dynamic [`Any`] trait object.
    ///
    /// This allows for mutable runtime type introspection and downcasting using methods like
    /// [`Any::downcast_mut`](std::any::Any::downcast_mut).
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

// Implement DynClone for the trait object itself.
dyn_clone::clone_trait_object!(ActonMessage);

/// Blanket implementation of `ActonMessage` for qualifying types.
///
/// Any type `T` that is `Any + Send + Sync + Debug + DynClone + 'static` automatically
/// implements `ActonMessage`. This simplifies defining custom message types.
impl<T> ActonMessage for T
where
    T: Any + Send + Sync + Debug + DynClone + 'static, // DynClone replaces Clone here
{
    /// Implementation of `as_any` for the blanket impl.
    #[inline]
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Implementation of `as_any_mut` for the blanket impl.
    #[inline]
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
