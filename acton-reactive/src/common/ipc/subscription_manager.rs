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

//! Subscription manager for IPC broker forwarding.
//!
//! This module tracks which IPC connections are subscribed to which message types,
//! allowing the IPC listener to forward broker broadcasts to interested clients.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

use super::types::IpcPushNotification;

/// Unique identifier for an IPC connection.
pub type ConnectionId = usize;

/// Channel sender for pushing notifications to a connection.
pub type PushSender = mpsc::Sender<IpcPushNotification>;

/// Credentials of the process on the other end of a Unix socket connection.
///
/// Supplied by the kernel when the connection is accepted, so they cannot be
/// forged by the peer. Applications can use them to make connection-level
/// authentication and access-control decisions.
///
/// # Choosing between the fields
///
/// Prefer [`uid`](Self::uid) and [`gid`](Self::gid) for authorization. A PID
/// identifies a process only for as long as that process lives: PIDs are
/// recycled, so a check that reads a PID and then acts on it can be defeated by
/// the original process exiting and its number being reused. The user and group
/// ids are fixed for the life of the connection and are the sound basis for a
/// policy decision. [`pid`](Self::pid) is best treated as a diagnostic — it is
/// what lets a log line name the process that connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerCredentials {
    /// Process ID of the peer, when the platform reported one.
    pid: Option<u32>,
    /// User ID of the peer.
    uid: u32,
    /// Group ID of the peer.
    gid: u32,
}

impl PeerCredentials {
    /// Build credentials from the raw values the platform reports.
    ///
    /// `pid` is signed at the OS level; a value that cannot be represented as a
    /// `u32` (a negative placeholder) is treated as "no PID reported" rather
    /// than being coerced into a nonsensical number.
    pub(crate) fn from_raw(pid: Option<i32>, uid: u32, gid: u32) -> Self {
        Self {
            pid: pid.and_then(|pid| u32::try_from(pid).ok()),
            uid,
            gid,
        }
    }

    /// Process ID of the peer, if the platform reported one.
    ///
    /// See the type-level note on why this is a diagnostic rather than an
    /// authorization primitive.
    #[must_use]
    pub const fn pid(self) -> Option<u32> {
        self.pid
    }

    /// User ID of the peer.
    #[must_use]
    pub const fn uid(self) -> u32 {
        self.uid
    }

    /// Group ID of the peer.
    #[must_use]
    pub const fn gid(self) -> u32 {
        self.gid
    }
}

impl std::fmt::Display for PeerCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.pid {
            Some(pid) => write!(f, "pid={pid} uid={} gid={}", self.uid, self.gid),
            None => write!(f, "pid=unknown uid={} gid={}", self.uid, self.gid),
        }
    }
}

/// Statistics for the subscription manager.
#[derive(Debug, Default)]
pub struct SubscriptionStats {
    /// Total subscriptions added.
    pub subscriptions_added: AtomicUsize,
    /// Total subscriptions removed.
    pub subscriptions_removed: AtomicUsize,
    /// Total push notifications sent.
    pub push_notifications_sent: AtomicUsize,
    /// Total push notifications dropped (channel full or closed).
    pub push_notifications_dropped: AtomicUsize,
}

impl SubscriptionStats {
    /// Get the number of subscriptions added.
    #[must_use]
    pub fn subscriptions_added(&self) -> usize {
        self.subscriptions_added.load(Ordering::Relaxed)
    }

    /// Get the number of subscriptions removed.
    #[must_use]
    pub fn subscriptions_removed(&self) -> usize {
        self.subscriptions_removed.load(Ordering::Relaxed)
    }

    /// Get the number of push notifications sent.
    #[must_use]
    pub fn push_notifications_sent(&self) -> usize {
        self.push_notifications_sent.load(Ordering::Relaxed)
    }

    /// Get the number of push notifications dropped.
    #[must_use]
    pub fn push_notifications_dropped(&self) -> usize {
        self.push_notifications_dropped.load(Ordering::Relaxed)
    }
}

/// Information about a subscribed connection.
struct ConnectionInfo {
    /// Channel for sending push notifications to this connection.
    push_sender: PushSender,
    /// Set of message type names this connection is subscribed to.
    subscribed_types: HashSet<String>,
    /// Credentials of the process behind this connection, when the platform
    /// reported them.
    peer: Option<PeerCredentials>,
}

/// Manages IPC connection subscriptions for broker forwarding.
///
/// This struct tracks which connections are subscribed to which message types
/// and provides methods to efficiently forward broker broadcasts to interested
/// connections.
///
/// # Thread Safety
///
/// This struct is designed to be shared across multiple tasks using `Arc`.
/// All operations are thread-safe.
pub struct SubscriptionManager {
    /// Maps connection ID to connection info (subscriptions and push channel).
    connections: DashMap<ConnectionId, ConnectionInfo>,
    /// Maps message type name to set of subscribed connection IDs.
    /// This is the primary index for fast lookup during broadcast forwarding.
    type_to_connections: DashMap<String, HashSet<ConnectionId>>,
    /// Maps `TypeId` to message type name for internal type routing.
    type_id_to_name: RwLock<HashMap<TypeId, String>>,
    /// Statistics.
    stats: SubscriptionStats,
}

impl Default for SubscriptionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SubscriptionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionManager")
            .field("connection_count", &self.connections.len())
            .field("subscribed_types_count", &self.type_to_connections.len())
            .field("type_id_mappings", &self.type_id_to_name.read().len())
            .field("stats", &self.stats)
            .finish()
    }
}

impl SubscriptionManager {
    /// Creates a new subscription manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
            type_to_connections: DashMap::new(),
            type_id_to_name: RwLock::new(HashMap::new()),
            stats: SubscriptionStats::default(),
        }
    }

    /// Returns a reference to the statistics.
    #[must_use]
    pub const fn stats(&self) -> &SubscriptionStats {
        &self.stats
    }

    /// Registers a connection with its push notification channel.
    ///
    /// This should be called when a new IPC connection is established.
    ///
    /// `peer` carries the kernel-reported credentials of the connecting process,
    /// or `None` when the platform did not report them. Callers with no interest
    /// in peer identity pass `None`.
    pub fn register_connection(
        &self,
        conn_id: ConnectionId,
        push_sender: PushSender,
        peer: Option<PeerCredentials>,
    ) {
        trace!(conn_id, "Registering connection for subscriptions");
        self.connections.insert(
            conn_id,
            ConnectionInfo {
                push_sender,
                subscribed_types: HashSet::new(),
                peer,
            },
        );
    }

    /// Credentials of the process behind a connection.
    ///
    /// Returns `None` when the connection is unknown or the platform did not
    /// report credentials for it.
    #[must_use]
    pub fn peer_credentials(&self, conn_id: ConnectionId) -> Option<PeerCredentials> {
        self.connections.get(&conn_id).and_then(|info| info.peer)
    }

    /// Process ID of the peer behind a connection.
    ///
    /// Convenience over [`peer_credentials`](Self::peer_credentials). Note that a
    /// PID is a diagnostic rather than an authorization primitive; see
    /// [`PeerCredentials`] for why `uid`/`gid` are the sound basis for a policy
    /// decision.
    #[must_use]
    pub fn peer_pid(&self, conn_id: ConnectionId) -> Option<u32> {
        self.peer_credentials(conn_id).and_then(PeerCredentials::pid)
    }

    /// Unregisters a connection, removing all its subscriptions.
    ///
    /// This should be called when an IPC connection is closed.
    pub fn unregister_connection(&self, conn_id: ConnectionId) {
        if let Some((_, info)) = self.connections.remove(&conn_id) {
            // Remove this connection from all message type indices
            for type_name in &info.subscribed_types {
                if let Some(mut entry) = self.type_to_connections.get_mut(type_name) {
                    entry.remove(&conn_id);
                    if entry.is_empty() {
                        // Clean up empty sets
                        drop(entry);
                        self.type_to_connections.remove(type_name);
                    }
                }
                self.stats
                    .subscriptions_removed
                    .fetch_add(1, Ordering::Relaxed);
            }
            debug!(
                conn_id,
                removed_subscriptions = info.subscribed_types.len(),
                "Unregistered connection and removed subscriptions"
            );
        }
    }

    /// Subscribes a connection to one or more message types.
    ///
    /// Returns the list of message types the connection is now subscribed to.
    pub fn subscribe(&self, conn_id: ConnectionId, message_types: &[String]) -> Vec<String> {
        let Some(mut conn_entry) = self.connections.get_mut(&conn_id) else {
            warn!(conn_id, "Cannot subscribe: connection not registered");
            return Vec::new();
        };

        for type_name in message_types {
            if conn_entry.subscribed_types.insert(type_name.clone()) {
                // Add to type index
                self.type_to_connections
                    .entry(type_name.clone())
                    .or_default()
                    .insert(conn_id);
                self.stats
                    .subscriptions_added
                    .fetch_add(1, Ordering::Relaxed);
                trace!(conn_id, message_type = %type_name, "Added subscription");
            }
        }

        conn_entry.subscribed_types.iter().cloned().collect()
    }

    /// Unsubscribes a connection from one or more message types.
    ///
    /// If `message_types` is empty, unsubscribes from all types.
    /// Returns the list of message types the connection is still subscribed to.
    pub fn unsubscribe(&self, conn_id: ConnectionId, message_types: &[String]) -> Vec<String> {
        let Some(mut conn_entry) = self.connections.get_mut(&conn_id) else {
            warn!(conn_id, "Cannot unsubscribe: connection not registered");
            return Vec::new();
        };

        if message_types.is_empty() {
            // Unsubscribe from all
            let types_to_remove: Vec<_> = conn_entry.subscribed_types.drain().collect();
            for type_name in &types_to_remove {
                if let Some(mut entry) = self.type_to_connections.get_mut(type_name) {
                    entry.remove(&conn_id);
                    if entry.is_empty() {
                        drop(entry);
                        self.type_to_connections.remove(type_name);
                    }
                }
                self.stats
                    .subscriptions_removed
                    .fetch_add(1, Ordering::Relaxed);
            }
            trace!(
                conn_id,
                count = types_to_remove.len(),
                "Unsubscribed from all types"
            );
            return Vec::new();
        }

        for type_name in message_types {
            if conn_entry.subscribed_types.remove(type_name) {
                if let Some(mut entry) = self.type_to_connections.get_mut(type_name) {
                    entry.remove(&conn_id);
                    if entry.is_empty() {
                        drop(entry);
                        self.type_to_connections.remove(type_name);
                    }
                }
                self.stats
                    .subscriptions_removed
                    .fetch_add(1, Ordering::Relaxed);
                trace!(conn_id, message_type = %type_name, "Removed subscription");
            }
        }

        conn_entry.subscribed_types.iter().cloned().collect()
    }

    /// Gets the list of message types a connection is subscribed to.
    #[must_use]
    pub fn get_subscriptions(&self, conn_id: ConnectionId) -> Vec<String> {
        self.connections
            .get(&conn_id)
            .map(|entry| entry.subscribed_types.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Registers a mapping from `TypeId` to message type name.
    ///
    /// This is used to route internal broker broadcasts (which use `TypeId`)
    /// to the correct message type name for subscription matching.
    pub fn register_type_mapping(&self, type_id: TypeId, type_name: String) {
        let mut map = self.type_id_to_name.write();
        map.insert(type_id, type_name);
    }

    /// Gets the message type name for a `TypeId`.
    #[must_use]
    pub fn get_type_name(&self, type_id: &TypeId) -> Option<String> {
        let map = self.type_id_to_name.read();
        map.get(type_id).cloned()
    }

    /// Forwards a push notification to all connections subscribed to the message type.
    ///
    /// Uses non-blocking `try_send` to avoid backpressure from slow consumers.
    pub fn forward_to_subscribers(&self, notification: &IpcPushNotification) {
        let message_type = &notification.message_type;

        let Some(connections_entry) = self.type_to_connections.get(message_type) else {
            trace!(message_type, "No subscribers for message type");
            return;
        };

        let conn_ids: Vec<_> = connections_entry.iter().copied().collect();
        drop(connections_entry); // Release lock before sending

        for conn_id in conn_ids {
            if let Some(conn_info) = self.connections.get(&conn_id) {
                let notification_clone = notification.clone();
                match conn_info.push_sender.try_send(notification_clone) {
                    Ok(()) => {
                        self.stats
                            .push_notifications_sent
                            .fetch_add(1, Ordering::Relaxed);
                        trace!(conn_id, message_type, "Forwarded push notification");
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        self.stats
                            .push_notifications_dropped
                            .fetch_add(1, Ordering::Relaxed);
                        warn!(
                            conn_id,
                            message_type, "Push channel full, dropping notification"
                        );
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        self.stats
                            .push_notifications_dropped
                            .fetch_add(1, Ordering::Relaxed);
                        trace!(conn_id, message_type, "Push channel closed");
                        // Connection will be cleaned up when it fully disconnects
                    }
                }
            }
        }
    }

    /// Returns the number of registered connections.
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Returns the number of unique message types with active subscriptions.
    #[must_use]
    pub fn subscribed_types_count(&self) -> usize {
        self.type_to_connections.len()
    }

    /// Returns the total number of subscriptions across all connections.
    #[must_use]
    pub fn total_subscriptions(&self) -> usize {
        self.type_to_connections
            .iter()
            .map(|entry| entry.value().len())
            .sum()
    }
}

/// A handle for sending push notifications to a specific connection.
///
/// This is given to the push notification forwarding task so it can
/// receive notifications and write them to the connection's stream.
pub struct PushReceiver {
    /// The connection ID, useful for debugging and logging.
    #[allow(dead_code)]
    pub conn_id: ConnectionId,
    /// The receiver for push notifications.
    pub receiver: mpsc::Receiver<IpcPushNotification>,
}

/// Creates a push notification channel for a connection.
///
/// Returns a sender (for the subscription manager) and a receiver (for the connection handler).
#[must_use]
pub fn create_push_channel(
    conn_id: ConnectionId,
    buffer_size: usize,
) -> (PushSender, PushReceiver) {
    let (sender, receiver) = mpsc::channel(buffer_size);
    (sender, PushReceiver { conn_id, receiver })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_subscription_manager_new() {
        let manager = SubscriptionManager::new();
        assert_eq!(manager.connection_count(), 0);
        assert_eq!(manager.subscribed_types_count(), 0);
    }

    #[test]
    fn test_register_unregister_connection() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);

        manager.register_connection(1, sender, None);
        assert_eq!(manager.connection_count(), 1);

        manager.unregister_connection(1);
        assert_eq!(manager.connection_count(), 0);
    }

    // ------------------------------------------------------------------
    // Peer credentials (issue #5)
    // ------------------------------------------------------------------

    #[test]
    fn a_reported_pid_is_carried_through() {
        let creds = PeerCredentials::from_raw(Some(4321), 1000, 1000);

        assert_eq!(creds.pid(), Some(4321));
        assert_eq!(creds.uid(), 1000);
        assert_eq!(creds.gid(), 1000);
    }

    /// A negative PID is a placeholder, not a process. Coercing it would invent
    /// a plausible-looking but wrong process id.
    #[test]
    fn a_negative_pid_is_treated_as_unreported() {
        let creds = PeerCredentials::from_raw(Some(-1), 1000, 1000);

        assert_eq!(creds.pid(), None);
        assert_eq!(creds.uid(), 1000, "uid survives an unusable pid");
    }

    #[test]
    fn an_absent_pid_stays_absent() {
        assert_eq!(PeerCredentials::from_raw(None, 0, 0).pid(), None);
    }

    #[test]
    fn root_credentials_are_representable() {
        let creds = PeerCredentials::from_raw(Some(1), 0, 0);

        assert_eq!(creds.pid(), Some(1));
        assert_eq!(creds.uid(), 0);
        assert_eq!(creds.gid(), 0);
    }

    #[test]
    fn credentials_render_for_logs() {
        assert_eq!(
            PeerCredentials::from_raw(Some(7), 1000, 20).to_string(),
            "pid=7 uid=1000 gid=20"
        );
        assert_eq!(
            PeerCredentials::from_raw(None, 1000, 20).to_string(),
            "pid=unknown uid=1000 gid=20"
        );
    }

    #[test]
    fn a_registered_connection_reports_its_peer() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);
        let creds = PeerCredentials::from_raw(Some(4321), 1000, 1000);

        manager.register_connection(1, sender, Some(creds));

        assert_eq!(manager.peer_credentials(1), Some(creds));
        assert_eq!(manager.peer_pid(1), Some(4321));
    }

    #[test]
    fn a_connection_registered_without_credentials_reports_none() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);

        manager.register_connection(1, sender, None);

        assert_eq!(manager.peer_credentials(1), None);
        assert_eq!(manager.peer_pid(1), None);
    }

    #[test]
    fn an_unknown_connection_has_no_peer() {
        let manager = SubscriptionManager::new();

        assert_eq!(manager.peer_credentials(99), None);
        assert_eq!(manager.peer_pid(99), None);
    }

    /// Credentials belong to the connection, so they must outlive subscription
    /// churn and vanish only when the connection does.
    #[test]
    fn credentials_survive_subscription_changes_and_end_with_the_connection() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);
        let creds = PeerCredentials::from_raw(Some(4321), 1000, 1000);

        manager.register_connection(1, sender, Some(creds));
        manager.subscribe(1, &["TypeA".to_string()]);
        assert_eq!(manager.peer_credentials(1), Some(creds));

        manager.unsubscribe(1, &["TypeA".to_string()]);
        assert_eq!(manager.peer_credentials(1), Some(creds));

        manager.unregister_connection(1);
        assert_eq!(manager.peer_credentials(1), None);
    }

    /// Each connection keeps its own peer; they must not bleed into each other.
    #[test]
    fn each_connection_keeps_its_own_peer() {
        let manager = SubscriptionManager::new();
        let (sender1, _r1) = mpsc::channel(10);
        let (sender2, _r2) = mpsc::channel(10);

        manager.register_connection(1, sender1, Some(PeerCredentials::from_raw(Some(11), 1, 1)));
        manager.register_connection(2, sender2, Some(PeerCredentials::from_raw(Some(22), 2, 2)));

        assert_eq!(manager.peer_pid(1), Some(11));
        assert_eq!(manager.peer_pid(2), Some(22));
    }

    #[test]
    fn test_subscribe_unsubscribe() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);

        manager.register_connection(1, sender, None);

        // Subscribe to some types
        let subscribed = manager.subscribe(1, &["TypeA".to_string(), "TypeB".to_string()]);
        assert_eq!(subscribed.len(), 2);
        assert!(subscribed.contains(&"TypeA".to_string()));
        assert!(subscribed.contains(&"TypeB".to_string()));

        assert_eq!(manager.subscribed_types_count(), 2);
        assert_eq!(manager.total_subscriptions(), 2);

        // Unsubscribe from one type
        let subscribed = manager.unsubscribe(1, &["TypeA".to_string()]);
        assert_eq!(subscribed.len(), 1);
        assert!(subscribed.contains(&"TypeB".to_string()));

        assert_eq!(manager.subscribed_types_count(), 1);
        assert_eq!(manager.total_subscriptions(), 1);

        // Unsubscribe from all
        let subscribed = manager.unsubscribe(1, &[]);
        assert!(subscribed.is_empty());
        assert_eq!(manager.subscribed_types_count(), 0);
    }

    #[test]
    fn test_unregister_cleans_subscriptions() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);

        manager.register_connection(1, sender, None);
        manager.subscribe(1, &["TypeA".to_string(), "TypeB".to_string()]);
        assert_eq!(manager.subscribed_types_count(), 2);

        manager.unregister_connection(1);
        assert_eq!(manager.subscribed_types_count(), 0);
    }

    #[test]
    fn test_multiple_connections_same_type() {
        let manager = SubscriptionManager::new();
        let (sender1, _receiver1) = mpsc::channel(10);
        let (sender2, _receiver2) = mpsc::channel(10);

        manager.register_connection(1, sender1, None);
        manager.register_connection(2, sender2, None);

        manager.subscribe(1, &["TypeA".to_string()]);
        manager.subscribe(2, &["TypeA".to_string()]);

        assert_eq!(manager.subscribed_types_count(), 1);
        assert_eq!(manager.total_subscriptions(), 2);

        // Unregister one connection
        manager.unregister_connection(1);
        assert_eq!(manager.subscribed_types_count(), 1);
        assert_eq!(manager.total_subscriptions(), 1);

        // Unregister the other
        manager.unregister_connection(2);
        assert_eq!(manager.subscribed_types_count(), 0);
    }

    #[tokio::test]
    async fn test_forward_to_subscribers() {
        let manager = Arc::new(SubscriptionManager::new());
        let (sender, mut receiver) = mpsc::channel(10);

        manager.register_connection(1, sender, None);
        manager.subscribe(1, &["PriceUpdate".to_string()]);

        let notification = IpcPushNotification::new(
            "PriceUpdate",
            Some("price_service".to_string()),
            serde_json::json!({ "price": 100.0 }),
        );

        manager.forward_to_subscribers(&notification);

        let notification_out = receiver.try_recv().unwrap();
        assert_eq!(notification_out.message_type, "PriceUpdate");
        assert_eq!(manager.stats().push_notifications_sent(), 1);
    }

    #[test]
    fn test_forward_no_subscribers() {
        let manager = Arc::new(SubscriptionManager::new());

        let notification =
            IpcPushNotification::new("UnsubscribedType", None, serde_json::json!({}));

        // Should not panic, just do nothing
        manager.forward_to_subscribers(&notification);
        assert_eq!(manager.stats().push_notifications_sent(), 0);
    }

    #[test]
    fn test_type_mapping() {
        struct TestMessage;

        let manager = SubscriptionManager::new();
        let type_id = TypeId::of::<TestMessage>();

        manager.register_type_mapping(type_id, "TestMessage".to_string());
        assert_eq!(
            manager.get_type_name(&type_id),
            Some("TestMessage".to_string())
        );
    }

    #[tokio::test]
    async fn test_create_push_channel() {
        let conn_id = 42;
        let buffer_size = 10;

        let (sender, receiver) = create_push_channel(conn_id, buffer_size);

        // Verify the receiver has the correct connection ID
        assert_eq!(receiver.conn_id, conn_id);

        // Test that we can send through the channel
        let notification = IpcPushNotification::new(
            "TestMessage",
            Some("test_actor".to_string()),
            serde_json::json!({ "test": true }),
        );

        sender.send(notification.clone()).await.unwrap();

        // Receive the notification
        let mut channel = receiver.receiver;
        let msg = channel.recv().await.unwrap();
        assert_eq!(msg.message_type, "TestMessage");
    }

    #[test]
    fn test_push_receiver_struct() {
        let (_, receiver) = create_push_channel(123, 5);
        assert_eq!(receiver.conn_id, 123);
    }
}
