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

//! Convenient boxed future replies for agents using old or new style handlers

use std::future::Future;
use std::pin::Pin;

/// A utility namespace for creating standard return types for `act_on` message handlers.
///
/// Message handlers registered with [`ManagedAgent::act_on`](crate::actor::ManagedAgent::act_on)
/// typically need to return a future that is boxed and pinned, specifically [`FutureBox`].
/// This struct provides helpers to create common future types that might be needed
/// as part of that process.
///
/// It acts purely as a namespace and is not intended to be instantiated.
pub struct AgentReply;

impl AgentReply {
    /// Creates an immediately resolving, no-operation future, boxed and pinned.
    ///
    /// This is useful for message handlers that perform synchronous work or do not need
    /// to perform any asynchronous operations after processing the message.
    ///
    /// # Returns
    ///
    /// A `Pin<Box<impl Future<Output=()>>>` that completes immediately. This can often
    /// be coerced or converted into the required [`FutureBox`].
    #[inline]
    #[must_use] 
    pub fn immediate() -> Pin<Box<impl Future<Output = ()> + Sized>> {
        // Original return type
        Box::pin(async move {})
    }

    /// Wraps an existing future into a `Pin<Box<F>>`.
    ///
    /// This method takes any future `F` with `Output=()` and boxes and pins it.
    /// This is often a necessary step before potentially casting or using it where
    /// a [`FutureBox`] is expected, provided `F` meets the `Send + Sync + 'static` bounds.
    ///
    /// # Type Parameters
    ///
    /// * `F`: The type of the input future. Must have `Output=()`.
    ///
    /// # Arguments
    ///
    /// * `future`: The future to be wrapped.
    ///
    /// # Returns
    ///
    /// A `Pin<Box<F>>` containing the provided future.
    #[inline]
    pub fn from_async<F>(future: F) -> Pin<Box<F>>
    // Original return type
    where
        F: Future<Output = ()> + Sized, // Original bounds
    {
        Box::pin(future)
    }
}
