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

//! Declares which requests can cross a process boundary, enabling
//! [`RemoteActorRef::ask`].
//!
//! [`RemoteActorRef::ask`]: crate::common::ipc::RemoteActorRef::ask

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::traits::Request;

/// A [`Request`] that can also be asked of an actor in **another process**.
///
/// [`Request`] alone is enough to [`ask`](crate::traits::ActorHandleInterface::ask) a local
/// actor, because a local reply travels as a `dyn` message over an in-process channel and
/// never needs a wire form. Crossing a process boundary does need one, in both directions —
/// hence the extra bounds here rather than on `Request`, which would tax every local-only
/// caller for a capability they never use.
///
/// # Why this is a separate trait
///
/// So that asking a remote actor for something unserializable is a **compile error** rather
/// than a call that appears to work and cannot. A message that only implements `Request` can
/// be asked of a local handle and nowhere else, and the compiler says so at the call site.
///
/// # The name is the contract
///
/// [`MESSAGE_TYPE`](Self::MESSAGE_TYPE) must match the name the **peer** registered this
/// type under:
///
/// ```rust,ignore
/// // In the server process:
/// runtime.ipc_registry().register::<GetCount>("GetCount");
/// ```
///
/// It is spelled out rather than derived from [`std::any::type_name`] because the two are
/// not the same thing and cannot be kept the same by the compiler. `type_name` yields a
/// path that changes when a type moves between modules, and the peer may be a different
/// binary, a different version of this crate, or not written in Rust at all. A wire name is
/// a protocol constant, so it is written down as one. A mismatch is reported as
/// [`AskError::PeerRejected`](crate::common::AskError::PeerRejected) with the peer's
/// `UNKNOWN_MESSAGE_TYPE` code, not silently ignored.
///
/// The reply type must be registered on the peer too. If it is not, the peer's listener
/// still answers, with a diagnostic fallback payload rather than the real reply; that
/// arrives as [`AskError::UnexpectedReply`](crate::common::AskError::UnexpectedReply)
/// carrying the payload, which names the unregistered type.
///
/// # Example
///
/// ```rust
/// # #[cfg(feature = "ipc")] {
/// use acton_reactive::prelude::*;
///
/// #[acton_message(ipc)]
/// struct GetCount;
///
/// #[acton_message(ipc)]
/// struct Count {
///     value: usize,
/// }
///
/// impl Request for GetCount {
///     type Response = Count;
/// }
///
/// impl RemoteRequest for GetCount {
///     const MESSAGE_TYPE: &'static str = "GetCount";
/// }
/// # }
/// ```
///
/// `#[acton_message(ipc)]` adds the `serde` derives; it does not register the type, which
/// stays an explicit step on whichever side receives the message.
pub trait RemoteRequest: Request + Serialize
where
    Self::Response: DeserializeOwned,
{
    /// The name this message is registered under on the peer.
    ///
    /// Must equal the string passed to
    /// [`IpcTypeRegistry::register`](crate::common::ipc::IpcTypeRegistry::register) in the
    /// receiving process.
    const MESSAGE_TYPE: &'static str;
}
