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

//! Request/reply support: the machinery behind
//! [`ask`](crate::traits::ActorHandleInterface::ask).
//!
//! # How a reply finds its way home
//!
//! A reply is routed by address, and an address is a channel sender: [`MessageAddress`]
//! pairs an `mpsc::Sender<Envelope>` with an identifier. Every handler registrar builds
//! its reply envelope by swapping the inbound envelope's `recipient` and `reply_to`, so
//! `ctx.reply_envelope().send(..)` always delivers to whatever address the *sender*
//! stamped on the request.
//!
//! `ask` exploits that: it mints a private one-slot channel, stamps the sending half on
//! the request as `reply_to`, and awaits the receiving half. Each call gets its own
//! channel, so correlation is structural — there is no correlation-id map to keep, and
//! two concurrent asks cannot be confused with one another. (The IPC client does keep
//! such a map, because it multiplexes many requests over a single socket. In-process
//! there is nothing to multiplex.)
//!
//! # Why this cannot hang
//!
//! Two mechanisms, layered, because neither covers the other's cases.
//!
//! **Channel closure — precise, and usually immediate.** [`exchange`] drops its own
//! copy of the reply sender *before* awaiting. From that moment the only surviving
//! senders live inside the [`Envelope`] handed to the actor, so when that envelope is
//! dropped every sender disappears at once and the receiver reports end-of-stream
//! rather than waiting. The envelope is dropped on every path that ends without a
//! reply:
//!
//! * the handler returns without replying — the common `Reply::ready()` case;
//! * the actor stops with the request on the far side of the shutdown boundary — a
//!   stopping actor drains what is already queued behind its stop signal, then closes
//!   its inbox, and anything past that point is let go;
//! * the handler panics, under either `catch-handler-panics` regime;
//! * the actor is restarted, which builds a wholly new mailbox and drops the old one.
//!
//! Each of those returns [`AskError::NoReply`] in microseconds, naming what happened.
//! Keeping a second sender alive across the await would silently defeat this, which is
//! what [`exchange`]'s explicit `drop` is there to prevent.
//!
//! **A deadline — the backstop.** Closure cannot help when the reply address is still
//! alive but no reply is coming anyway: an actor wedged in a handler that never
//! returns, a handler that stores its reply envelope and then forgets it, or a caller
//! that deadlocked itself by asking from inside a mutable handler (see the `# Deadlock`
//! section on [`ask`]). Those hold a live sender indefinitely, so only a clock ends
//! them. [`DEFAULT_ASK_TIMEOUT`] bounds every `ask`, matching `IpcClient::request`,
//! which pairs a default deadline with an explicit `_with_timeout` variant.
//!
//! The layering matters: the deadline is a backstop, not the primary mechanism. An
//! `ask` that fails for one of the ordinary reasons above returns promptly and says
//! which one, instead of stalling until a timer expires.
//!
//! [`ask`]: crate::traits::ActorHandleInterface::ask

use std::any::type_name;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{instrument, trace};

use crate::message::{Envelope, MessageAddress, MessageError, OutboundEnvelope};
use crate::traits::{ActonMessage, Request};

/// The reply channel holds one slot, because `ask` awaits exactly one answer.
const REPLY_CHANNEL_CAPACITY: usize = 1;

/// How long [`ask`](crate::traits::ActorHandleInterface::ask) waits before giving up.
///
/// Thirty seconds, matching the IPC client's own default request timeout, so that a
/// local and a remote request behave alike rather than needing two mental models.
///
/// This is a backstop for the cases channel closure cannot detect — a wedged actor, a
/// forgotten reply envelope, a self-inflicted deadlock — not the ordinary path. A
/// request whose reply address is dropped fails in microseconds with a specific error
/// and never waits for this deadline. Use
/// [`ask_with_timeout`](crate::traits::ActorHandleInterface::ask_with_timeout) where a
/// different bound is wanted.
pub const DEFAULT_ASK_TIMEOUT: Duration = Duration::from_secs(30);

/// Why an [`ask`](crate::traits::ActorHandleInterface::ask) did not produce a reply.
///
/// Every variant is a terminal answer: `ask` resolves to one of these rather than
/// waiting indefinitely. The type is `#[non_exhaustive]`, so further variants can be
/// added without a breaking change; match with a `_` arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AskError {
    /// The request could never be delivered, because the actor's inbox was already
    /// closed.
    ///
    /// The actor had stopped, or was past the point of accepting messages, before the
    /// request was enqueued. No handler ran, so nothing was left half-done.
    Undeliverable,

    /// Delivery was abandoned because the cancellation token fired.
    ///
    /// The surrounding scope is shutting down. As with
    /// [`Undeliverable`](Self::Undeliverable), the request never reached a handler.
    Cancelled,

    /// The request was delivered, but no reply will ever arrive.
    ///
    /// Every address that could have carried the answer has been dropped. The causes
    /// are indistinguishable from the caller's side, and deliberately not guessed at
    /// here:
    ///
    /// * the handler returned without sending a reply — entirely legal, and what any
    ///   handler ending in `Reply::ready()` does;
    /// * the actor stopped without reaching the request — a stopping actor drains what
    ///   is queued behind its stop signal and then closes its inbox, so a request past
    ///   that boundary is let go;
    /// * the handler panicked before replying;
    /// * the actor was restarted, abandoning the mailbox the request sat in.
    ///
    /// What they share is the only thing a caller can act on: no answer is coming.
    NoReply,

    /// The deadline expired with the reply still outstanding.
    ///
    /// Distinct from [`NoReply`](Self::NoReply), and the distinction is worth acting
    /// on: `NoReply` means the actor demonstrably let go of the request, while this
    /// means it still holds a live reply address and simply has not answered. Causes
    /// are an actor wedged in a handler that never returns, a handler that stored its
    /// reply envelope and never used it, or a caller that deadlocked itself by asking
    /// from inside a mutable handler.
    ///
    /// The request may still be processed after this is returned; nothing is rolled
    /// back.
    TimedOut {
        /// How long the caller waited before giving up.
        after: Duration,
    },

    /// The handler replied with a type other than the one
    /// [`Request::Response`](crate::traits::Request::Response) names.
    ///
    /// The handler and the [`Request`] implementation disagree. This is reported rather
    /// than silently discarded, because the alternative is an `ask` that looks like a
    /// lost reply for a reason that is really a bug in the actor.
    UnexpectedReply {
        /// The type the request declares as its reply.
        expected: &'static str,
        /// The reply the handler actually sent, rendered with [`Debug`](fmt::Debug).
        ///
        /// A rendering rather than a type name: the concrete type behind a trait object
        /// cannot be named on stable Rust, and the value identifies the culprit at
        /// least as well.
        received: String,
    },
}

impl fmt::Display for AskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Undeliverable => write!(
                f,
                "the request could not be delivered: the actor's inbox is closed"
            ),
            Self::Cancelled => write!(f, "the request was cancelled before delivery"),
            Self::NoReply => write!(
                f,
                "the request was delivered but no reply will arrive: the handler did not \
                 reply, or the actor stopped, panicked, or was restarted first"
            ),
            Self::TimedOut { after } => write!(
                f,
                "no reply within {after:?}: the actor still holds the request but has \
                 not answered"
            ),
            Self::UnexpectedReply { expected, received } => write!(
                f,
                "the request declares its reply type as `{expected}`, but the handler \
                 sent {received}"
            ),
        }
    }
}

impl std::error::Error for AskError {}

impl From<MessageError> for AskError {
    /// Maps a delivery failure onto the matching ask failure.
    ///
    /// Only the pre-delivery outcomes are reachable here: once a request is in the
    /// inbox, every later failure surfaces through the reply channel instead.
    fn from(error: MessageError) -> Self {
        match error {
            MessageError::Cancelled => Self::Cancelled,
            _ => Self::Undeliverable,
        }
    }
}

/// Decides what a received reply means, without performing any I/O.
///
/// Separated from the async path so the interesting judgement — did an answer arrive,
/// and is it the answer the request asked for — can be tested by calling a function and
/// comparing a value, with no runtime and no actors. `None` stands for "the reply
/// channel closed", which is how the no-hang guarantee reports itself.
pub fn classify_reply<R>(
    reply: Option<Arc<dyn ActonMessage + Send + Sync>>,
) -> Result<R, AskError>
where
    R: ActonMessage + Clone,
{
    let Some(message) = reply else {
        return Err(AskError::NoReply);
    };

    // `&*message`, not `message`. `ActonMessage` has a blanket impl covering every
    // qualifying type, and `Arc<dyn ActonMessage>` qualifies — so `message.as_any()`
    // resolves against the `Arc` and yields an `Any` whose type is the `Arc`, never the
    // message inside it. Every downcast would then fail while the payload looked
    // perfectly correct in logs. Dereferencing first is what reaches the real message.
    ActonMessage::as_any(&*message)
        .downcast_ref::<R>()
        .cloned()
        .ok_or_else(|| AskError::UnexpectedReply {
            expected: type_name::<R>(),
            received: format!("{message:?}"),
        })
}

/// Sends `request` to `recipient` and awaits its reply, giving up after `timeout`.
///
/// The deadline covers the whole exchange, delivery included: a full inbox makes
/// delivery itself wait, and a caller asked for a bound on the operation rather than on
/// one phase of it.
#[instrument(
    skip(request, recipient, cancellation_token),
    fields(request_type = type_name::<R>())
)]
pub async fn send_request<R>(
    recipient: MessageAddress,
    cancellation_token: CancellationToken,
    request: R,
    timeout: Duration,
) -> Result<R::Response, AskError>
where
    R: Request,
{
    tokio::time::timeout(
        timeout,
        exchange::<R>(recipient, cancellation_token, request),
    )
    .await
    .unwrap_or(Err(AskError::TimedOut { after: timeout }))
}

/// Performs the request/reply exchange, with no deadline of its own.
///
/// Split out from [`send_request`] so the deadline wraps the operation as a whole. This
/// is where the channel-closure guarantee lives; see the module documentation for why
/// the `drop` below is load-bearing.
async fn exchange<R>(
    recipient: MessageAddress,
    cancellation_token: CancellationToken,
    request: R,
) -> Result<R::Response, AskError>
where
    R: Request,
{
    // A handler that replies twice finds the channel closed on the second attempt,
    // which is logged and discarded: `ask` promises exactly one answer.
    let (reply_sender, mut reply_receiver) = mpsc::channel::<Envelope>(REPLY_CHANNEL_CAPACITY);

    let reply_address = MessageAddress::new(reply_sender, reply_identifier());
    let envelope =
        OutboundEnvelope::new_with_recipient(reply_address, recipient, cancellation_token);

    envelope.try_send(request).await?;

    // Load-bearing. `envelope` still holds a clone of the reply sender; while it lives
    // the channel cannot close, so a request that is never answered would wait forever.
    // Dropping it here leaves the actor's copy as the only one, which makes the reply
    // channel close exactly when the actor lets go of the request.
    drop(envelope);

    let reply = reply_receiver.recv().await.map(|envelope| envelope.message);
    trace!(replied = reply.is_some(), "ask completed");

    classify_reply::<R::Response>(reply)
}

/// Mints the identifier carried by an `ask` reply address.
///
/// For logs and diagnostics only — replies are routed by channel, never by name — so a
/// failure to build a decorated identifier is not worth propagating to the caller.
fn reply_identifier() -> acton_ern::Ern {
    acton_ern::Ern::with_root("ask-reply").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Count {
        value: usize,
    }

    #[derive(Debug, Clone)]
    struct SomethingElse;

    /// A closed reply channel is the signal that no answer is coming. This is the case
    /// that makes `ask` terminate rather than hang, so it is asserted directly and not
    /// only through the actor tests.
    #[test]
    fn a_closed_reply_channel_is_reported_as_no_reply() {
        let outcome = classify_reply::<Count>(None);
        assert_eq!(outcome, Err(AskError::NoReply));
    }

    #[test]
    fn a_reply_of_the_declared_type_is_returned_to_the_caller() {
        let reply: Arc<dyn ActonMessage + Send + Sync> = Arc::new(Count { value: 7 });
        let outcome = classify_reply::<Count>(Some(reply));
        assert_eq!(outcome, Ok(Count { value: 7 }));
    }

    /// A handler answering with the wrong type must not be mistaken for a lost reply.
    #[test]
    fn a_reply_of_another_type_is_reported_rather_than_discarded() {
        let reply: Arc<dyn ActonMessage + Send + Sync> = Arc::new(SomethingElse);

        match classify_reply::<Count>(Some(reply)) {
            Err(AskError::UnexpectedReply { expected, received }) => {
                assert!(
                    expected.ends_with("Count"),
                    "the expected type should name the declared reply, got `{expected}`"
                );
                assert!(
                    received.contains("SomethingElse"),
                    "the rendering should identify what was actually sent, got `{received}`"
                );
            }
            other => panic!("expected UnexpectedReply, got {other:?}"),
        }
    }

    /// Delivery failures happen before any handler runs, so they must not be flattened
    /// into `NoReply`: the caller can tell "never ran" from "ran, said nothing".
    #[test]
    fn delivery_failures_keep_their_identity() {
        assert_eq!(AskError::from(MessageError::Cancelled), AskError::Cancelled);
        assert_eq!(
            AskError::from(MessageError::ChannelClosed),
            AskError::Undeliverable
        );
        assert_eq!(
            AskError::from(MessageError::SendFailed("closed".to_owned())),
            AskError::Undeliverable
        );
    }

    #[test]
    fn every_variant_describes_itself() {
        let variants = [
            AskError::Undeliverable,
            AskError::Cancelled,
            AskError::NoReply,
            AskError::UnexpectedReply {
                expected: "Count",
                received: "SomethingElse".to_owned(),
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
