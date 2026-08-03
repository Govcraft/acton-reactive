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

//! Declares which reply a message expects, enabling [`ask`].
//!
//! [`ask`]: crate::traits::ActorHandleInterface::ask

use crate::traits::acton_message::ActonMessage;

/// A message that expects exactly one reply of a known type.
///
/// Implementing this trait is what makes a message usable with
/// [`ActorHandleInterface::ask`](crate::traits::ActorHandleInterface::ask). The
/// associated [`Response`](Request::Response) type names the reply, so callers write
/// `handle.ask(GetCount).await?` and receive a `Count` with no turbofish and no
/// runtime type argument.
///
/// # Why an associated type
///
/// Pinning the answer to the request type means a mismatched pair is a compile error
/// rather than a runtime surprise, and it is what lets inference carry the reply type
/// through `?`. The cost is deliberate: one request type has exactly one reply type.
/// If the same payload needs two different answers in two different contexts, define
/// two request types — that ambiguity is worth making visible.
///
/// # Example
///
/// ```
/// use acton_reactive::prelude::*;
///
/// #[acton_message]
/// struct GetCount;
///
/// #[acton_message]
/// struct Count {
///     value: usize,
/// }
///
/// impl Request for GetCount {
///     type Response = Count;
/// }
/// ```
///
/// The handler answers through the reply envelope, exactly as it always has — `ask`
/// adds no new obligation on the handler side:
///
/// ```ignore
/// actor.act_on::<GetCount>(|actor, ctx| {
///     let reply = ctx.reply_envelope();
///     let value = actor.model.count;
///     Reply::pending(async move { reply.send(Count { value }).await })
/// });
/// ```
///
/// A handler that returns without replying is legal; the caller receives
/// [`AskError::NoReply`](crate::common::AskError::NoReply) rather than waiting forever.
pub trait Request: ActonMessage {
    /// The reply this request expects.
    ///
    /// The handler must send exactly this type through the reply envelope it is
    /// given. Sending some other type is reported to the caller as
    /// [`AskError::UnexpectedReply`](crate::common::AskError::UnexpectedReply).
    type Response: ActonMessage + Clone;
}
