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
    pub fn register_connection(&self, conn_id: ConnectionId, push_sender: PushSender) {
        trace!(conn_id, "Registering connection for subscriptions");
        self.connections.insert(
            conn_id,
            ConnectionInfo {
                push_sender,
                subscribed_types: HashSet::new(),
            },
        );
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
                self.stats.subscriptions_removed.fetch_add(1, Ordering::Relaxed);
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
    pub fn subscribe(
        &self,
        conn_id: ConnectionId,
        message_types: &[String],
    ) -> Vec<String> {
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
                self.stats.subscriptions_added.fetch_add(1, Ordering::Relaxed);
                trace!(conn_id, message_type = %type_name, "Added subscription");
            }
        }

        conn_entry.subscribed_types.iter().cloned().collect()
    }

    /// Unsubscribes a connection from one or more message types.
    ///
    /// If `message_types` is empty, unsubscribes from all types.
    /// Returns the list of message types the connection is still subscribed to.
    pub fn unsubscribe(
        &self,
        conn_id: ConnectionId,
        message_types: &[String],
    ) -> Vec<String> {
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
                self.stats.subscriptions_removed.fetch_add(1, Ordering::Relaxed);
            }
            trace!(conn_id, count = types_to_remove.len(), "Unsubscribed from all types");
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
                self.stats.subscriptions_removed.fetch_add(1, Ordering::Relaxed);
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
                        self.stats.push_notifications_sent.fetch_add(1, Ordering::Relaxed);
                        trace!(conn_id, message_type, "Forwarded push notification");
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        self.stats.push_notifications_dropped.fetch_add(1, Ordering::Relaxed);
                        warn!(conn_id, message_type, "Push channel full, dropping notification");
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        self.stats.push_notifications_dropped.fetch_add(1, Ordering::Relaxed);
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
pub fn create_push_channel(conn_id: ConnectionId, buffer_size: usize) -> (PushSender, PushReceiver) {
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

        manager.register_connection(1, sender);
        assert_eq!(manager.connection_count(), 1);

        manager.unregister_connection(1);
        assert_eq!(manager.connection_count(), 0);
    }

    #[test]
    fn test_subscribe_unsubscribe() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);

        manager.register_connection(1, sender);

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

        manager.register_connection(1, sender);
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

        manager.register_connection(1, sender1);
        manager.register_connection(2, sender2);

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

        manager.register_connection(1, sender);
        manager.subscribe(1, &["PriceUpdate".to_string()]);

        let notification = IpcPushNotification::new(
            "PriceUpdate",
            Some("price_service".to_string()),
            serde_json::json!({ "price": 100.0 }),
        );

        manager.forward_to_subscribers(&notification);

        let received = receiver.try_recv().unwrap();
        assert_eq!(received.message_type, "PriceUpdate");
        assert_eq!(manager.stats().push_notifications_sent(), 1);
    }

    #[test]
    fn test_forward_no_subscribers() {
        let manager = Arc::new(SubscriptionManager::new());

        let notification = IpcPushNotification::new(
            "UnsubscribedType",
            None,
            serde_json::json!({}),
        );

        // Should not panic, just do nothing
        manager.forward_to_subscribers(&notification);
        assert_eq!(manager.stats().push_notifications_sent(), 0);
    }

    #[test]
    fn test_type_mapping() {
        let manager = SubscriptionManager::new();

        struct TestMessage;
        let type_id = TypeId::of::<TestMessage>();

        manager.register_type_mapping(type_id, "TestMessage".to_string());
        assert_eq!(manager.get_type_name(&type_id), Some("TestMessage".to_string()));
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
            Some("test_agent".to_string()),
            serde_json::json!({ "test": true }),
        );

        sender.send(notification.clone()).await.unwrap();

        // Receive the notification
        let mut rx = receiver.receiver;
        let received = rx.recv().await.unwrap();
        assert_eq!(received.message_type, "TestMessage");
    }

    #[test]
    fn test_push_receiver_struct() {
        let (_, receiver) = create_push_channel(123, 5);
        assert_eq!(receiver.conn_id, 123);
    }
}
