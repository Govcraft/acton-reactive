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

//! [`ask`](RemoteActorRef::ask) across a process boundary.
//!
//! The local [`ask`](crate::traits::ActorHandleInterface::ask) mints a private channel and
//! awaits it. That cannot cross a process boundary, so this module does the same job over
//! the transport [`IpcClient`] already provides — correlation ids, a pending-request map,
//! and a deadline — and presents it as the same call:
//!
//! ```rust,ignore
//! let count: Count = handle.ask(GetCount).await?;          // local
//! let count: Count = remote.ask(GetCount).await?;          // remote
//! ```
//!
//! No second transport is introduced here. This is a typed façade over
//! [`IpcClient::request_with_timeout`], plus the judgement about what a response means.
//!
//! # Why this cannot hang
//!
//! The local `ask` layers two mechanisms, and the remote path must not settle for one.
//!
//! **The connection closing — precise, and usually immediate.** [`IpcClient`] registers a
//! one-shot for the correlation id *before* writing the frame. If the connection drops, the
//! reader task drops the sending half, and the caller is woken at once with
//! [`AskError::TransportFailed`] rather than waiting out the clock. This is the remote
//! counterpart of the local reply channel closing when the actor lets go of the request.
//!
//! **A deadline — the backstop.** For everything closure cannot see: a peer that accepted
//! the request and went quiet, an actor wedged in a handler.
//!
//! # Three clocks, deliberately aligned
//!
//! A remote request passes three places that could each give up at a different time:
//!
//! 1. the caller's deadline, on [`IpcClient::request_with_timeout`];
//! 2. the peer listener's wait on the actor, from the envelope's `response_timeout_ms`;
//! 3. [`IpcClient`]'s own default, which applies only when [`request`] is used instead.
//!
//! [`ask_with_timeout`] stamps its deadline on **both** 1 and 2 rather than letting the
//! envelope keep its independent 30s default. Otherwise a caller asking for a two-second
//! bound would leave the peer holding its response proxy open for thirty, and a short
//! deadline would silently stop bounding the work rather than just the wait.
//!
//! [`request`]: IpcClient::request
//! [`ask_with_timeout`]: RemoteActorRef::ask_with_timeout

use std::any::type_name;
use std::fmt;
use std::time::Duration;

use serde::de::DeserializeOwned;
use tracing::{instrument, trace};

use crate::common::ask::{AskError, DEFAULT_ASK_TIMEOUT};
use crate::common::ipc::client::IpcClient;
use crate::common::ipc::types::{IpcEnvelope, IpcError, IpcResponse, NO_REPLY_MESSAGE};
use crate::traits::RemoteRequest;

/// The peer's error code for a request whose deadline expired while the actor held it.
const TIMEOUT_CODE: &str = "TIMEOUT";

/// The peer's error code shared by genuine I/O failures and the no-reply case.
const IO_ERROR_CODE: &str = "IO_ERROR";

/// An actor in another process, addressed by the name it was exposed under.
///
/// Obtained from [`IpcClient::actor`]. Borrows the client, so it costs nothing beyond the
/// target name and cannot outlive the connection it speaks over.
///
/// Its [`ask`](Self::ask) is deliberately the same call as the local one:
///
/// ```rust,ignore
/// let count: Count = handle.ask(GetCount).await?;   // local actor
/// let count: Count = remote.ask(GetCount).await?;   // actor in another process
/// ```
///
/// The difference is in the bounds, not the shape. A remote request must be able to travel,
/// so [`ask`](Self::ask) takes a [`RemoteRequest`] — [`Request`] plus a wire form and a wire
/// name — where the local one takes a bare [`Request`]. A message that cannot cross the
/// boundary therefore fails to compile here rather than appearing to work.
///
/// [`Request`]: crate::traits::Request
#[derive(Clone, Copy)]
pub struct RemoteActorRef<'client> {
    client: &'client IpcClient,
    target: &'client str,
}

/// Shows the actor addressed, which is the only part worth seeing.
///
/// Hand-written because [`IpcClient`] is not [`Debug`](fmt::Debug), and because a
/// connection's internals would bury the one field a reader is looking for.
impl fmt::Debug for RemoteActorRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteActorRef")
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl<'client> RemoteActorRef<'client> {
    /// Names an actor in the peer process.
    pub(crate) const fn new(client: &'client IpcClient, target: &'client str) -> Self {
        Self { client, target }
    }

    /// The name this reference addresses.
    #[must_use]
    pub const fn target(&self) -> &str {
        self.target
    }

    /// Sends a request to the remote actor and waits for its reply.
    ///
    /// The counterpart of [`ActorHandleInterface::ask`] for an actor in another process,
    /// and deliberately the same call to write:
    ///
    /// ```rust,ignore
    /// let count: Count = remote.ask(GetCount).await?;
    /// ```
    ///
    /// The peer's handler answers through its reply envelope exactly as a local handler
    /// does, and cannot tell a remote `ask` from a local one — the listener gives it an
    /// ordinary reply address and forwards whatever comes back.
    ///
    /// # What is addressable
    ///
    /// **Exactly one actor, named by the string it was exposed under** with
    /// [`ActorRuntime::ipc_expose`](crate::common::ActorRuntime::ipc_expose) in the peer
    /// process.
    ///
    /// * *Not the broker.* `ask` has no meaning over
    ///   [`broadcast`](crate::traits::Broadcaster::broadcast), remotely for the same reason
    ///   it has none locally: a broadcast has no single replier, so there is no one answer
    ///   to wait for. This addresses the actor named here, and nothing else.
    /// * *Not an unregistered type.* Both the request and the reply must be registered on
    ///   the peer with [`IpcTypeRegistry::register`](crate::common::ipc::IpcTypeRegistry::register).
    ///   An unregistered request is refused with [`AskError::PeerRejected`]; an unregistered
    ///   reply comes back as [`AskError::UnexpectedReply`], because the peer answers with a
    ///   diagnostic payload rather than the real value.
    ///
    /// # Deadlock
    ///
    /// The local warning against asking from inside a `mutate_on` handler applies here too,
    /// and for the same reason — a mutable handler is awaited inline on the actor's own
    /// message loop, so waiting for any reply inside one stops the actor. Crossing a process
    /// boundary does not change that; it only makes the other party harder to see.
    ///
    /// # How this resolves when no reply comes
    ///
    /// It always resolves. The connection closing wakes the caller at once; the deadline
    /// backstops a peer that accepted the request and went quiet. See the module docs.
    ///
    /// # Errors
    ///
    /// * [`AskError::PeerRejected`] — the peer refused before dispatch (no such actor, no
    ///   such message type, busy, rate-limited, shutting down). Nothing ran.
    /// * [`AskError::NoReply`] — the peer's handler returned without replying.
    /// * [`AskError::TimedOut`] — the deadline expired with the reply outstanding, at
    ///   either end.
    /// * [`AskError::TransportFailed`] — the connection failed; whether the request was
    ///   processed is unknown.
    /// * [`AskError::UnexpectedReply`] — the reply did not deserialize into
    ///   [`Request::Response`](crate::traits::Request::Response).
    /// * [`AskError::Undeliverable`] — the request could not be serialized, so nothing was
    ///   sent.
    ///
    /// [`ActorHandleInterface::ask`]: crate::traits::ActorHandleInterface::ask
    pub async fn ask<R>(&self, request: R) -> Result<R::Response, AskError>
    where
        R: RemoteRequest,
        R::Response: DeserializeOwned,
    {
        self.ask_with_timeout(request, DEFAULT_ASK_TIMEOUT).await
    }

    /// [`ask`](Self::ask) with an explicit deadline instead of [`DEFAULT_ASK_TIMEOUT`].
    ///
    /// The deadline bounds the whole exchange and is also stamped on the request so the
    /// peer stops waiting on its actor at the same moment; see the module docs on why the
    /// two are tied together.
    ///
    /// Every caveat on [`ask`](Self::ask) applies unchanged.
    ///
    /// # Errors
    ///
    /// As [`ask`](Self::ask).
    // `actor`, not `target`: `target` is reserved by `tracing`'s macro syntax.
    #[instrument(
        skip(self, request),
        fields(actor = self.target, message_type = R::MESSAGE_TYPE)
    )]
    pub async fn ask_with_timeout<R>(
        &self,
        request: R,
        timeout: Duration,
    ) -> Result<R::Response, AskError>
    where
        R: RemoteRequest,
        R::Response: DeserializeOwned,
    {
        // Serializing a value the caller already owns fails only for shapes JSON cannot
        // express. Nothing reaches the wire, so this is a delivery failure rather than a
        // reply failure.
        let payload = serde_json::to_value(&request).map_err(|_| AskError::Undeliverable)?;

        let envelope = IpcEnvelope::new_request_with_timeout(
            self.target,
            R::MESSAGE_TYPE,
            payload,
            timeout_millis(timeout),
        );

        trace!(actor = self.target, "Asking remote actor and awaiting its reply");

        match self.client.request_with_timeout(envelope, timeout).await {
            Ok(response) => classify_remote_response::<R::Response>(response, timeout),
            Err(error) => Err(classify_ipc_error(&error, timeout)),
        }
    }
}

/// Converts a deadline to the milliseconds the wire carries, saturating rather than
/// wrapping.
///
/// A `Duration` outstrips `u64` milliseconds only past a half-billion years, but wrapping
/// there would turn the longest possible deadline into the shortest, so it saturates.
fn timeout_millis(timeout: Duration) -> u64 {
    u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
}

/// Decides what a peer's response means, without performing any I/O.
///
/// Separated from the async path for the same reason
/// [`classify_reply`](crate::common::ask::classify_reply) is: the interesting judgement —
/// did an answer arrive, is it the answer the request asked for, and if not whose fault is
/// it — becomes a function call and a value comparison, with no socket and no runtime.
pub fn classify_remote_response<R>(
    response: IpcResponse,
    timeout: Duration,
) -> Result<R, AskError>
where
    R: DeserializeOwned,
{
    if !response.success {
        return Err(classify_failure(
            response.error_code.as_deref(),
            response.error.as_deref(),
            timeout,
        ));
    }

    let Some(payload) = response.payload else {
        // A success with nothing in it is the peer saying it has no answer for us.
        return Err(AskError::NoReply);
    };

    // Borrowing the payload rather than consuming it: the error path needs it, and
    // `&Value` is itself a deserializer, so this costs no clone.
    R::deserialize(&payload).map_err(|_| AskError::UnexpectedReply {
        expected: type_name::<R>(),
        // The raw payload, for the reason the local variant gives a rendering rather than
        // a type name: it identifies the culprit at least as well. When the peer never
        // registered its reply type this payload is the listener's fallback blob, which
        // carries `_ipc_fallback` and the offending type name outright.
        received: payload.to_string(),
    })
}

/// Maps a failed [`IpcResponse`] onto the ask failure it represents.
///
/// The split is drawn on what a caller can act on, not on how the peer phrased it:
/// refused-before-dispatch (safe to retry), the actor said nothing (definite), the deadline
/// expired (the actor may still be working), or the link itself failed (unknown).
fn classify_failure(code: Option<&str>, detail: Option<&str>, timeout: Duration) -> AskError {
    let detail = detail.unwrap_or("the peer reported a failure without describing it");

    match code {
        // The peer's own wait on its actor expired. The actor received the request and
        // did not answer in time, which is exactly what the local `TimedOut` means.
        Some(TIMEOUT_CODE) => AskError::TimedOut { after: timeout },

        // `IO_ERROR` is overloaded by the listener: it covers real I/O faults and the
        // benign case of a handler returning without replying. Only the message separates
        // them, so it is pinned by a shared constant.
        Some(IO_ERROR_CODE) if detail.contains(NO_REPLY_MESSAGE) => AskError::NoReply,
        Some(IO_ERROR_CODE) => AskError::TransportFailed {
            detail: detail.to_owned(),
        },

        // Everything else the peer names is a refusal made before the actor ran.
        other => AskError::PeerRejected {
            code: other.map(ToOwned::to_owned),
            detail: detail.to_owned(),
        },
    }
}

/// Maps a client-side transport failure onto the ask failure it represents.
pub fn classify_ipc_error(error: &IpcError, timeout: Duration) -> AskError {
    match error {
        IpcError::Timeout => AskError::TimedOut { after: timeout },

        // Refusals the client itself can report without a round trip, plus those the peer
        // sent. None of them ran the actor.
        IpcError::ActorNotFound(_)
        | IpcError::UnknownMessageType(_)
        | IpcError::TargetBusy
        | IpcError::RateLimited { .. }
        | IpcError::ShuttingDown
        | IpcError::ConnectionLimitReached { .. }
        | IpcError::UnsupportedProtocolVersion { .. } => AskError::PeerRejected {
            code: None,
            detail: error.to_string(),
        },

        // The request may or may not have been processed; that is the whole point of the
        // variant.
        IpcError::ConnectionClosed | IpcError::IoError(_) | IpcError::ProtocolError(_) => {
            AskError::TransportFailed {
                detail: error.to_string(),
            }
        }

        // Serialization failed locally, so nothing reached the wire.
        IpcError::SerializationError(_) => AskError::Undeliverable,
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Count {
        value: usize,
    }

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn ask(response: IpcResponse) -> Result<Count, AskError> {
        classify_remote_response::<Count>(response, TEST_TIMEOUT)
    }

    #[test]
    fn a_reply_of_the_declared_type_is_returned_to_the_caller() {
        let response = IpcResponse::success("c1", Some(serde_json::json!({ "value": 7 })));
        assert_eq!(ask(response), Ok(Count { value: 7 }));
    }

    /// A peer that answers "yes" with nothing in hand has no answer for us. Folding this
    /// into a deserialization failure would blame the reply type for a missing reply.
    #[test]
    fn a_success_carrying_no_payload_is_reported_as_no_reply() {
        let response = IpcResponse::success("c1", None);
        assert_eq!(ask(response), Err(AskError::NoReply));
    }

    #[test]
    fn a_reply_of_another_shape_is_reported_rather_than_discarded() {
        let response = IpcResponse::success("c1", Some(serde_json::json!({ "wrong": true })));

        match ask(response) {
            Err(AskError::UnexpectedReply { expected, received }) => {
                assert!(
                    expected.ends_with("Count"),
                    "the expected type should name the declared reply, got `{expected}`"
                );
                assert!(
                    received.contains("wrong"),
                    "the raw payload should identify what arrived, got `{received}`"
                );
            }
            other => panic!("expected UnexpectedReply, got {other:?}"),
        }
    }

    /// The peer answers even when it cannot serialize the reply properly, with a fallback
    /// blob naming the type nobody registered. That name is the one actionable thing in
    /// the whole failure, so it must survive into the error.
    #[test]
    fn an_unregistered_reply_type_names_itself_in_the_error() {
        let response = IpcResponse::success(
            "c1",
            Some(serde_json::json!({
                "_ipc_fallback": true,
                "type": "Count",
                "debug": "Count { value: 7 }",
            })),
        );

        match ask(response) {
            Err(AskError::UnexpectedReply { received, .. }) => {
                assert!(
                    received.contains("_ipc_fallback"),
                    "the fallback marker should survive, got `{received}`"
                );
            }
            other => panic!("expected UnexpectedReply, got {other:?}"),
        }
    }

    /// The peer's own wait on its actor expiring means the actor got the request and did
    /// not answer — the local `TimedOut` situation exactly. Reporting it as a refusal
    /// would claim nothing ran, which is the opposite of what happened.
    #[test]
    fn a_peer_side_deadline_is_a_timeout_not_a_refusal() {
        let response =
            IpcResponse::error_with_message("c1", "TIMEOUT", "Request timed out after 5000 ms");
        assert_eq!(ask(response), Err(AskError::TimedOut { after: TEST_TIMEOUT }));
    }

    /// The one case the overloaded `IO_ERROR` code cannot distinguish on its own. A
    /// handler returning without replying is ordinary and definite; reporting it as a
    /// transport failure would tell the caller delivery was uncertain when it was not.
    #[test]
    fn a_silent_handler_is_no_reply_rather_than_a_transport_failure() {
        let response = IpcResponse::error(
            "c1",
            &IpcError::IoError(NO_REPLY_MESSAGE.to_owned()),
        );
        assert_eq!(ask(response), Err(AskError::NoReply));
    }

    #[test]
    fn a_genuine_io_failure_remains_a_transport_failure() {
        let response =
            IpcResponse::error("c1", &IpcError::IoError("broken pipe".to_owned()));

        match ask(response) {
            Err(AskError::TransportFailed { detail }) => {
                assert!(
                    detail.contains("broken pipe"),
                    "the transport's own words should survive, got `{detail}`"
                );
            }
            other => panic!("expected TransportFailed, got {other:?}"),
        }
    }

    /// A refusal made before dispatch carries the peer's code, which is what tells a
    /// mistyped actor name from an unregistered message type.
    #[test]
    fn a_refusal_before_dispatch_keeps_the_peers_code() {
        let response =
            IpcResponse::error("c1", &IpcError::ActorNotFound("counter".to_owned()));

        match ask(response) {
            Err(AskError::PeerRejected { code, detail }) => {
                assert_eq!(code.as_deref(), Some("ACTOR_NOT_FOUND"));
                assert!(
                    detail.contains("counter"),
                    "the detail should name the actor asked for, got `{detail}`"
                );
            }
            other => panic!("expected PeerRejected, got {other:?}"),
        }
    }

    #[test]
    fn a_failure_the_peer_did_not_describe_is_still_terminal() {
        let response = IpcResponse {
            correlation_id: "c1".to_owned(),
            success: false,
            error: None,
            error_code: None,
            payload: None,
        };

        match ask(response) {
            Err(AskError::PeerRejected { code, .. }) => assert_eq!(code, None),
            other => panic!("expected PeerRejected, got {other:?}"),
        }
    }

    #[test]
    fn a_client_side_deadline_maps_to_the_timeout_error() {
        assert_eq!(
            classify_ipc_error(&IpcError::Timeout, TEST_TIMEOUT),
            AskError::TimedOut { after: TEST_TIMEOUT }
        );
    }

    /// The distinction the whole variant exists for: a broken link cannot claim the
    /// request never ran, because it might have.
    #[test]
    fn a_broken_connection_never_claims_the_request_was_not_processed() {
        for error in [
            IpcError::ConnectionClosed,
            IpcError::IoError("reset".to_owned()),
            IpcError::ProtocolError("short frame".to_owned()),
        ] {
            assert!(
                matches!(
                    classify_ipc_error(&error, TEST_TIMEOUT),
                    AskError::TransportFailed { .. }
                ),
                "{error:?} leaves delivery uncertain and must say so"
            );
        }
    }

    #[test]
    fn refusals_that_never_reached_an_actor_are_reported_as_such() {
        for error in [
            IpcError::ActorNotFound("nobody".to_owned()),
            IpcError::UnknownMessageType("Nope".to_owned()),
            IpcError::TargetBusy,
            IpcError::RateLimited { retry_after_ms: 10 },
            IpcError::ShuttingDown,
            IpcError::ConnectionLimitReached { limit: 4 },
        ] {
            assert!(
                matches!(
                    classify_ipc_error(&error, TEST_TIMEOUT),
                    AskError::PeerRejected { .. }
                ),
                "{error:?} ran no handler and must say so"
            );
        }
    }

    /// Nothing reached the wire, so this is a delivery failure rather than a lost reply.
    #[test]
    fn a_request_that_cannot_be_serialized_is_undeliverable() {
        assert_eq!(
            classify_ipc_error(&IpcError::SerializationError("nan".to_owned()), TEST_TIMEOUT),
            AskError::Undeliverable
        );
    }

    /// A deadline beyond `u64` milliseconds must not wrap into a tiny one, which would
    /// turn the longest possible wait into the shortest.
    #[test]
    fn an_unrepresentable_deadline_saturates_rather_than_wrapping() {
        assert_eq!(timeout_millis(Duration::from_secs(5)), 5_000);
        assert_eq!(timeout_millis(Duration::MAX), u64::MAX);
    }

    #[test]
    fn every_new_variant_describes_itself() {
        let variants = [
            AskError::PeerRejected {
                code: Some("ACTOR_NOT_FOUND".to_owned()),
                detail: "Actor not found: counter".to_owned(),
            },
            AskError::PeerRejected {
                code: None,
                detail: "unexplained".to_owned(),
            },
            AskError::TransportFailed {
                detail: "broken pipe".to_owned(),
            },
        ];

        for variant in variants {
            assert!(
                !variant.to_string().is_empty(),
                "{variant:?} must render a message"
            );
        }
    }
}
