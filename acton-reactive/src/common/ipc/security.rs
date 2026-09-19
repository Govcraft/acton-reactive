//! Application-defined IPC admission, authorization, and trusted connection identity.
//!
//! Identity comes from server admission and is never read from a client payload.
//! Authorization callbacks run without subscription routing locks and must be fast
//! and nonblocking. Admission may await application state through its returned future.

use std::any::Any;
use std::future::Future;
use std::panic::RefUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::subscription_manager::{ConnectionId, PeerCredentials};
use super::types::{IpcEnvelope, IpcPushNotification};
use crate::traits::ActonMessage;

/// Error code for an operation denied by the server's security policy.
pub const IPC_ACCESS_DENIED_CODE: &str = "ACCESS_DENIED";

/// An application identity established by server-side admission.
///
/// Identity values are opaque to the framework and are not serialized or printed.
/// Store established identity data here and keep mutable permission state in the
/// policy. Values must be `RefUnwindSafe`, preserving handler context unwind safety.
#[derive(Clone)]
pub struct IpcIdentity(Arc<dyn Any + Send + Sync + RefUnwindSafe>);

impl IpcIdentity {
    /// Stores an application-defined identity.
    #[must_use]
    pub fn new<T: Any + Send + Sync + RefUnwindSafe>(value: T) -> Self {
        Self(Arc::new(value))
    }

    /// Borrows the identity when its concrete type matches `T`.
    #[must_use]
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        let identity: &dyn Any = self.0.as_ref();
        identity.downcast_ref()
    }
}

impl std::fmt::Debug for IpcIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcIdentity").finish_non_exhaustive()
    }
}

/// Kernel-reported connection information supplied to admission.
#[derive(Clone, Copy, Debug)]
pub struct IpcConnectionInfo {
    connection_id: ConnectionId,
    peer: Option<PeerCredentials>,
}

impl IpcConnectionInfo {
    pub(crate) const fn new(connection_id: ConnectionId, peer: Option<PeerCredentials>) -> Self {
        Self {
            connection_id,
            peer,
        }
    }

    /// The connection identifier within this listener.
    #[must_use]
    pub const fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    /// Credentials reported by the operating system, if available.
    ///
    /// A policy requiring credentials must reject `None`. A PID can be recycled;
    /// applications using PID-based admission must manage the process lifetime.
    #[must_use]
    pub const fn peer_credentials(&self) -> Option<PeerCredentials> {
        self.peer
    }
}

#[derive(Debug)]
struct IpcSession {
    info: IpcConnectionInfo,
    identity: IpcIdentity,
    cancellation: CancellationToken,
}

/// Trusted context bound to one admitted connection for its lifetime.
///
/// Clones refer to the same session, including its revocation state. Context is
/// available to handlers of incoming IPC messages. Ordinary actor sends and
/// broadcasts do not inherit it; applications must explicitly define delegation.
#[derive(Clone, Debug)]
pub struct IpcConnectionContext(Arc<IpcSession>);

impl IpcConnectionContext {
    pub(crate) fn new(
        info: IpcConnectionInfo,
        identity: IpcIdentity,
        cancellation: CancellationToken,
    ) -> Self {
        Self(Arc::new(IpcSession {
            info,
            identity,
            cancellation,
        }))
    }

    /// Borrows the application's admitted identity as its concrete type.
    #[must_use]
    pub fn identity<T: Any>(&self) -> Option<&T> {
        self.0.identity.downcast_ref()
    }

    /// Credentials captured when this connection was accepted.
    #[must_use]
    pub fn peer_credentials(&self) -> Option<PeerCredentials> {
        self.0.info.peer_credentials()
    }

    /// The connection identifier within this listener.
    #[must_use]
    pub fn connection_id(&self) -> ConnectionId {
        self.0.info.connection_id()
    }

    /// Whether this session has been revoked or closed.
    ///
    /// Revocation does not roll back handlers that have already begun executing.
    #[must_use]
    pub fn is_revoked(&self) -> bool {
        self.0.cancellation.is_cancelled()
    }

    pub(crate) fn cancellation_token(&self) -> &CancellationToken {
        &self.0.cancellation
    }

    pub(crate) fn same_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// An operation presented to an application's authorization policy.
///
/// Request payloads and selectors are untrusted client input. The context passed
/// alongside them contains the separately established trusted identity.
#[derive(Debug)]
#[non_exhaustive]
pub enum IpcOperation<'a> {
    /// Fire-and-forget, request/reply, or stream request, distinguished by its flags.
    Request(&'a IpcEnvelope),
    /// Add exact subscriptions as one atomic operation.
    Subscribe(&'a [String]),
    /// Remove exact subscriptions; an empty list removes all subscriptions.
    Unsubscribe(&'a [String]),
    /// Add prefix patterns as one atomic operation.
    SubscribePatterns(&'a [String]),
    /// Remove prefix patterns; an empty list removes all patterns only.
    UnsubscribePatterns(&'a [String]),
    /// Discover the listener's exposed actors and registered message types.
    Discover,
    /// Deliver a broker notification to this connection, including cached matches.
    Deliver(&'a IpcPushNotification),
}

/// An admission or authorization denial with a client-facing explanation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpcAccessDenied(String);

impl IpcAccessDenied {
    /// Creates a denial. Use a message safe to disclose to the client.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for IpcAccessDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for IpcAccessDenied {}

/// A cancellable asynchronous admission decision.
pub type IpcAdmission<'a> =
    Pin<Box<dyn Future<Output = Result<IpcIdentity, IpcAccessDenied>> + Send + 'a>>;

/// Application-defined connection admission and operation authorization.
///
/// Both decisions are required when opting into a policy. Existing listener
/// entry points remain permissive. Authorization must be fast and nonblocking;
/// it may be called directly by synchronous broker forwarding. Implementations
/// should keep mutable permissions in application-owned synchronized state.
/// Policies must be `RefUnwindSafe` to preserve existing manager unwind safety.
///
/// # Example
///
/// ```
/// use acton_reactive::ipc::{IpcAccessDenied, IpcAdmission, IpcConnectionContext,
///     IpcConnectionInfo, IpcIdentity, IpcOperation, IpcSecurityPolicy};
///
/// struct LocalUsers;
/// impl IpcSecurityPolicy for LocalUsers {
///     fn admit(&self, connection: IpcConnectionInfo) -> IpcAdmission<'_> {
///         Box::pin(async move {
///             let peer = connection.peer_credentials()
///                 .ok_or_else(|| IpcAccessDenied::new("Peer credentials required"))?;
///             // A real policy can await a process registry lookup here.
///             Ok(IpcIdentity::new(peer.uid()))
///         })
///     }
///
///     fn authorize(&self, context: &IpcConnectionContext, operation: IpcOperation<'_>)
///         -> Result<(), IpcAccessDenied>
///     {
///         let uid = context.identity::<u32>()
///             .ok_or_else(|| IpcAccessDenied::new("Unknown identity"))?;
///         match operation {
///             IpcOperation::Discover if *uid == 1000 => Ok(()),
///             _ => Err(IpcAccessDenied::new("Operation not permitted")),
///         }
///     }
/// }
/// ```
pub trait IpcSecurityPolicy: Send + Sync + RefUnwindSafe + 'static {
    /// Admits a connection using kernel-reported peer information.
    fn admit(&self, connection: IpcConnectionInfo) -> IpcAdmission<'_>;

    /// Authorizes an operation for the trusted admitted identity.
    ///
    /// Delivery decisions are evaluated for every notification, never cached.
    ///
    /// # Errors
    /// Returns a denial when this identity may not perform the operation.
    fn authorize(
        &self,
        context: &IpcConnectionContext,
        operation: IpcOperation<'_>,
    ) -> Result<(), IpcAccessDenied>;
}

#[derive(Clone, Debug)]
struct RoutedIpcMessage {
    payload: Arc<dyn ActonMessage + Send + Sync>,
    context: IpcConnectionContext,
}

pub fn wrap_message(
    message: Box<dyn ActonMessage + Send + Sync>,
    context: IpcConnectionContext,
) -> Box<dyn ActonMessage + Send + Sync> {
    Box::new(RoutedIpcMessage {
        payload: Arc::from(message),
        context,
    })
}

pub fn message_context(message: &dyn ActonMessage) -> Option<&IpcConnectionContext> {
    message
        .as_any()
        .downcast_ref::<RoutedIpcMessage>()
        .map(|routed| &routed.context)
}

pub fn message_payload(message: &dyn ActonMessage) -> &dyn ActonMessage {
    message
        .as_any()
        .downcast_ref::<RoutedIpcMessage>()
        .map_or(message, |routed| routed.payload.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MessageContext;
    use crate::prelude::*;
    use std::time::Duration;
    use tokio::sync::mpsc;

    #[derive(Clone, Debug)]
    struct Probe<const KIND: usize>(mpsc::UnboundedSender<(usize, Option<String>)>);

    #[acton_actor]
    struct Recorder;

    fn record<const KIND: usize>(context: &MessageContext<Probe<KIND>>, error: bool) {
        let identity = context
            .ipc_context()
            .and_then(IpcConnectionContext::identity::<String>)
            .cloned();
        context
            .message()
            .0
            .send((KIND + usize::from(error) * 10, identity))
            .unwrap();
    }

    fn session(id: usize, identity: &str) -> IpcConnectionContext {
        IpcConnectionContext::new(
            IpcConnectionInfo::new(id, None),
            IpcIdentity::new(identity.to_owned()),
            CancellationToken::new(),
        )
    }

    struct CredentialsRequired;

    impl IpcSecurityPolicy for CredentialsRequired {
        fn admit(&self, connection: IpcConnectionInfo) -> IpcAdmission<'_> {
            Box::pin(async move {
                connection
                    .peer_credentials()
                    .map(|peer| IpcIdentity::new(peer.uid()))
                    .ok_or_else(|| IpcAccessDenied::new("Credentials required"))
            })
        }

        fn authorize(
            &self,
            _: &IpcConnectionContext,
            _: IpcOperation<'_>,
        ) -> Result<(), IpcAccessDenied> {
            Err(IpcAccessDenied::new("Not authorized"))
        }
    }

    #[tokio::test]
    async fn admission_can_reject_missing_kernel_credentials() {
        let denied = CredentialsRequired
            .admit(IpcConnectionInfo::new(1, None))
            .await;
        assert_eq!(denied.unwrap_err().to_string(), "Credentials required");
        let identity = CredentialsRequired
            .admit(IpcConnectionInfo::new(
                2,
                Some(PeerCredentials::from_raw(Some(10), 123, 456)),
            ))
            .await
            .unwrap();
        assert_eq!(identity.downcast_ref::<u32>(), Some(&123));
    }

    #[test]
    fn identity_is_opaque_and_context_clones_track_the_same_revocation() {
        let context = session(1, "private identity");
        let cloned = context.clone();
        assert_eq!(
            context.identity::<String>().map(String::as_str),
            Some("private identity")
        );
        assert!(context.identity::<u32>().is_none());
        assert!(!format!("{context:?}").contains("private identity"));
        assert!(context.same_session(&cloned));
        assert!(!context.same_session(&session(1, "private identity")));
        context.cancellation_token().cancel();
        assert!(cloned.is_revoked());
    }

    #[tokio::test]
    async fn every_handler_family_and_error_handler_receive_only_the_bound_identity() {
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = runtime.new_actor::<Recorder>();
        actor.mutate_on_sync::<Probe<0>>(|_, context| record(context, false));
        actor.act_on_sync::<Probe<1>>(|_, context| record(context, false));
        actor.mutate_on::<Probe<2>>(|_, context| {
            record(context, false);
            Reply::ready()
        });
        actor.act_on::<Probe<3>>(|_, context| {
            record(context, false);
            Reply::ready()
        });
        actor.try_mutate_on::<Probe<4>, (), IpcAccessDenied>(|_, context| {
            record(context, false);
            Box::pin(async { Err(IpcAccessDenied::new("test failure")) })
        });
        actor.try_act_on::<Probe<5>, (), IpcAccessDenied>(|_, context| {
            record(context, false);
            Box::pin(async { Err(IpcAccessDenied::new("test failure")) })
        });
        actor.on_error::<Probe<4>, IpcAccessDenied>(|_, context, _| {
            record(context, true);
            Reply::ready()
        });
        actor.on_error::<Probe<5>, IpcAccessDenied>(|_, context, _| {
            record(context, true);
            Reply::ready()
        });
        let handle = actor.start().await;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        send_probes(&handle, &sender, Some(&session(1, "first")));
        send_probes(&handle, &sender, Some(&session(2, "second")));
        send_probes(&handle, &sender, None);
        let mut observations = Vec::new();
        for _ in 0..24 {
            observations.push(
                tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        for identity in [Some("first".to_owned()), Some("second".to_owned()), None] {
            for kind in [0, 1, 2, 3, 4, 5, 14, 15] {
                assert_eq!(
                    observations
                        .iter()
                        .filter(|observation| **observation == (kind, identity.clone()))
                        .count(),
                    1
                );
            }
        }
        runtime.shutdown_all().await.unwrap();
    }

    fn send_probes(
        handle: &ActorHandle,
        sender: &mpsc::UnboundedSender<(usize, Option<String>)>,
        context: Option<&IpcConnectionContext>,
    ) {
        let probes: [Box<dyn ActonMessage + Send + Sync>; 6] = [
            Box::new(Probe::<0>(sender.clone())),
            Box::new(Probe::<1>(sender.clone())),
            Box::new(Probe::<2>(sender.clone())),
            Box::new(Probe::<3>(sender.clone())),
            Box::new(Probe::<4>(sender.clone())),
            Box::new(Probe::<5>(sender.clone())),
        ];
        for probe in probes {
            let message = if let Some(context) = context {
                wrap_message(probe, context.clone())
            } else {
                probe
            };
            handle.try_send_boxed(message).unwrap();
        }
    }

    #[tokio::test]
    async fn wrapped_terminate_preserves_framework_lifecycle_behavior() {
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = runtime.new_actor::<Recorder>();
        let (stopped, mut receiver) = mpsc::unbounded_channel();
        actor.after_stop(move |_| {
            stopped.send(()).unwrap();
            Reply::ready()
        });
        let handle = actor.start().await;
        handle
            .try_send_boxed(wrap_message(
                Box::new(SystemSignal::Terminate),
                session(1, "admitted"),
            ))
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .unwrap(),
            Some(())
        );
        runtime.shutdown_all().await.unwrap();
    }

    #[derive(Clone, Debug)]
    struct Hold {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    #[tokio::test]
    async fn queued_revoked_messages_are_skipped_before_handler_dispatch() {
        let mut runtime = ActonApp::launch_async().await;
        let mut actor = runtime.new_actor::<Recorder>();
        actor.mutate_on::<Hold>(|_, context| {
            let hold = context.message().clone();
            Reply::pending(async move {
                hold.entered.notify_one();
                hold.release.notified().await;
            })
        });
        actor.mutate_on_sync::<Probe<0>>(|_, context| record(context, false));
        let handle = actor.start().await;
        let hold = Hold {
            entered: Arc::default(),
            release: Arc::default(),
        };
        handle.send(hold.clone()).await;
        tokio::time::timeout(Duration::from_secs(5), hold.entered.notified())
            .await
            .unwrap();
        let context = session(1, "revoked");
        let (sender, mut receiver) = mpsc::unbounded_channel();
        handle
            .try_send_boxed(wrap_message(
                Box::new(Probe::<0>(sender.clone())),
                context.clone(),
            ))
            .unwrap();
        context.cancellation_token().cancel();
        handle.send(Probe::<0>(sender)).await;
        hold.release.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), receiver.recv())
                .await
                .unwrap(),
            Some((0, None))
        );
        runtime.shutdown_all().await.unwrap();
        assert!(receiver.try_recv().is_err());
    }
}
