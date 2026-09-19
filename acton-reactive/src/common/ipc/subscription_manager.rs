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

use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{trace, warn};

use super::security::{IpcConnectionContext, IpcOperation, IpcSecurityPolicy};
use super::types::IpcPushNotification;

/// Unique identifier for an IPC connection.
pub type ConnectionId = usize;

/// Channel sender for pushing notifications to a connection.
pub type PushSender = mpsc::Sender<IpcPushNotification>;

/// A case-sensitive IPC name prefix selector ending in exactly one `*`.
///
/// `*` matches every IPC name. All characters before the final star are literal,
/// including separators, question marks, and brackets. Selectors are limited to
/// 256 UTF-8 bytes. Exact subscriptions retain their literal semantics.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubscriptionPattern(String);

impl SubscriptionPattern {
    /// Returns the original selector.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Tests an IPC name against this selector.
    #[must_use]
    pub fn matches(&self, name: &str) -> bool {
        name.starts_with(&self.0[..self.0.len() - 1])
    }
}

impl std::fmt::Display for SubscriptionPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for SubscriptionPattern {
    type Error = PatternSubscriptionError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() > 256 {
            return Err(PatternSubscriptionError::PatternTooLong);
        }
        let Some(prefix) = value.strip_suffix('*') else {
            return Err(PatternSubscriptionError::InvalidPattern);
        };
        if prefix.contains('*') {
            return Err(PatternSubscriptionError::InvalidPattern);
        }
        Ok(Self(value))
    }
}

impl std::str::FromStr for SubscriptionPattern {
    type Err = PatternSubscriptionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value.to_owned())
    }
}

/// Failure to validate or apply a pattern subscription batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PatternSubscriptionError {
    /// A selector must end in exactly one wildcard.
    InvalidPattern,
    /// A selector exceeds 256 UTF-8 bytes.
    PatternTooLong,
    /// A request or a connection exceeds the limit of 128 patterns.
    TooManyPatterns,
    /// The connection has not been registered.
    UnknownConnection,
}

impl std::fmt::Display for PatternSubscriptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidPattern => "pattern must contain exactly one terminal '*'",
            Self::PatternTooLong => "pattern exceeds 256 UTF-8 bytes",
            Self::TooManyPatterns => "pattern limit of 128 exceeded",
            Self::UnknownConnection => "connection is not registered",
        })
    }
}

impl std::error::Error for PatternSubscriptionError {}

fn validate_patterns(
    values: &[String],
) -> Result<HashSet<SubscriptionPattern>, PatternSubscriptionError> {
    if values.len() > 128 {
        return Err(PatternSubscriptionError::TooManyPatterns);
    }
    values.iter().map(|value| value.parse()).collect()
}

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
    /// Total push notifications dropped (authorization denied, channel full or closed).
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
    incarnation: Arc<()>,
    cancellation: CancellationToken,
    security: Option<(IpcConnectionContext, Arc<dyn IpcSecurityPolicy>)>,
    /// Channel for sending push notifications to this connection.
    push_sender: PushSender,
    /// Set of message type names this connection is subscribed to.
    subscribed_types: HashSet<String>,
    subscribed_patterns: HashSet<SubscriptionPattern>,
    /// Credentials of the process behind this connection, when the platform
    /// reported them.
    peer: Option<PeerCredentials>,
}

struct DeliveryCandidate {
    conn_id: ConnectionId,
    incarnation: Arc<()>,
    context: IpcConnectionContext,
    policy: Arc<dyn IpcSecurityPolicy>,
}

impl ConnectionInfo {
    fn subscription_count(&self) -> usize {
        self.subscribed_types.len() + self.subscribed_patterns.len()
    }

    fn matches(&self, name: &str) -> bool {
        self.subscribed_types.contains(name)
            || self
                .subscribed_patterns
                .iter()
                .any(|pattern| pattern.matches(name))
    }
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
    // Lock order: state, then routes. Keep state locked through cache use and
    // nonblocking delivery so completed mutations cannot leave stale routes.
    state: RwLock<SubscriptionState>,
    routes: RwLock<HashMap<String, Arc<[ConnectionId]>>>,
    type_id_to_name: RwLock<HashMap<TypeId, String>>,
    stats: SubscriptionStats,
}

#[derive(Default)]
struct SubscriptionState {
    connections: HashMap<ConnectionId, ConnectionInfo>,
    exact: HashMap<String, HashSet<ConnectionId>>,
    patterns: HashMap<SubscriptionPattern, HashSet<ConnectionId>>,
}

fn remove_index<K: Eq + std::hash::Hash>(
    index: &mut HashMap<K, HashSet<ConnectionId>>,
    key: &K,
    id: ConnectionId,
) {
    if let Some(ids) = index.get_mut(key) {
        ids.remove(&id);
        if ids.is_empty() {
            index.remove(key);
        }
    }
}

impl SubscriptionState {
    fn remove_connection(&mut self, id: ConnectionId) -> Option<ConnectionInfo> {
        let info = self.connections.remove(&id)?;
        info.cancellation.cancel();
        for name in &info.subscribed_types {
            remove_index(&mut self.exact, name, id);
        }
        for pattern in &info.subscribed_patterns {
            remove_index(&mut self.patterns, pattern, id);
        }
        Some(info)
    }
}

impl Default for SubscriptionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SubscriptionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.read();
        f.debug_struct("SubscriptionManager")
            .field("connection_count", &state.connections.len())
            .field("subscribed_types_count", &state.exact.len())
            .field("subscribed_patterns_count", &state.patterns.len())
            .field("cached_routes", &self.routes.read().len())
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
            state: RwLock::new(SubscriptionState::default()),
            routes: RwLock::new(HashMap::new()),
            type_id_to_name: RwLock::new(HashMap::new()),
            stats: SubscriptionStats::default(),
        }
    }

    /// Returns a reference to the statistics, counting exact and pattern selectors.
    #[must_use]
    pub const fn stats(&self) -> &SubscriptionStats {
        &self.stats
    }

    /// Registers a connection and its kernel-reported peer credentials.
    /// Replacing an existing connection cancels it and removes its subscriptions.
    pub fn register_connection(
        &self,
        conn_id: ConnectionId,
        push_sender: PushSender,
        peer: Option<PeerCredentials>,
    ) {
        self.register_connection_with_token(conn_id, push_sender, peer, CancellationToken::new());
    }

    pub(crate) fn register_connection_with_token(
        &self,
        conn_id: ConnectionId,
        push_sender: PushSender,
        peer: Option<PeerCredentials>,
        cancellation: CancellationToken,
    ) {
        self.insert_connection(
            conn_id,
            ConnectionInfo {
                incarnation: Arc::new(()),
                cancellation,
                security: None,
                push_sender,
                peer,
                subscribed_types: HashSet::new(),
                subscribed_patterns: HashSet::new(),
            },
        );
    }

    pub(crate) fn register_authenticated_connection(
        &self,
        conn_id: ConnectionId,
        push_sender: PushSender,
        context: IpcConnectionContext,
        policy: Arc<dyn IpcSecurityPolicy>,
    ) {
        self.insert_connection(
            conn_id,
            ConnectionInfo {
                incarnation: Arc::new(()),
                cancellation: context.cancellation_token().clone(),
                peer: context.peer_credentials(),
                security: Some((context, policy)),
                push_sender,
                subscribed_types: HashSet::new(),
                subscribed_patterns: HashSet::new(),
            },
        );
    }

    fn insert_connection(&self, conn_id: ConnectionId, info: ConnectionInfo) {
        let mut state = self.state.write();
        let removed = state.remove_connection(conn_id);
        self.stats.subscriptions_removed.fetch_add(
            removed
                .as_ref()
                .map_or(0, ConnectionInfo::subscription_count),
            Ordering::Relaxed,
        );
        state.connections.insert(conn_id, info);
        self.routes.write().clear();
        drop(state);
        drop(removed);
    }

    /// Returns the admitted security context, if this is an authenticated connection.
    /// A retained context observes revocation even after the connection is removed.
    #[must_use]
    pub fn connection_context(&self, conn_id: ConnectionId) -> Option<IpcConnectionContext> {
        self.state
            .read()
            .connections
            .get(&conn_id)
            .and_then(|info| info.security.as_ref().map(|(context, _)| context.clone()))
    }

    /// Cancels a connection and removes all of its subscriptions.
    ///
    /// Returns whether the connection existed. Once this returns, no subsequent
    /// notification can be enqueued for that connection incarnation. Notifications
    /// already queued or written cannot be recalled. Listener tasks observe the
    /// cancellation token and close the transport.
    pub fn revoke_connection(&self, conn_id: ConnectionId) -> bool {
        let mut state = self.state.write();
        let existed = state.connections.contains_key(&conn_id);
        let removed = state.remove_connection(conn_id);
        self.stats.subscriptions_removed.fetch_add(
            removed
                .as_ref()
                .map_or(0, ConnectionInfo::subscription_count),
            Ordering::Relaxed,
        );
        self.routes.write().clear();
        drop(state);
        drop(removed);
        existed
    }

    /// Returns kernel-reported credentials, or `None` for unknown connections.
    #[must_use]
    pub fn peer_credentials(&self, conn_id: ConnectionId) -> Option<PeerCredentials> {
        self.state
            .read()
            .connections
            .get(&conn_id)
            .and_then(|info| info.peer)
    }

    /// Returns the peer process ID, a diagnostic rather than an authorization primitive.
    #[must_use]
    pub fn peer_pid(&self, conn_id: ConnectionId) -> Option<u32> {
        self.peer_credentials(conn_id)
            .and_then(PeerCredentials::pid)
    }

    /// Unregisters a connection, removing all exact and pattern subscriptions.
    pub fn unregister_connection(&self, conn_id: ConnectionId) {
        let mut state = self.state.write();
        let removed = state.remove_connection(conn_id);
        self.stats.subscriptions_removed.fetch_add(
            removed
                .as_ref()
                .map_or(0, ConnectionInfo::subscription_count),
            Ordering::Relaxed,
        );
        self.routes.write().clear();
        drop(state);
        drop(removed);
    }

    /// Adds literal message type names and returns the current exact subscriptions.
    pub fn subscribe(&self, conn_id: ConnectionId, message_types: &[String]) -> Vec<String> {
        let mut state = self.state.write();
        let SubscriptionState {
            connections, exact, ..
        } = &mut *state;
        let Some(info) = connections.get_mut(&conn_id) else {
            return Vec::new();
        };
        for name in message_types {
            if info.subscribed_types.insert(name.clone()) {
                exact.entry(name.clone()).or_default().insert(conn_id);
                self.stats
                    .subscriptions_added
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.routes.write().clear();
        let subscriptions = info.subscribed_types.iter().cloned().collect();
        drop(state);
        subscriptions
    }

    /// Removes literal subscriptions and returns remaining exact subscriptions.
    /// An empty batch removes both exact and pattern subscriptions.
    pub fn unsubscribe(&self, conn_id: ConnectionId, message_types: &[String]) -> Vec<String> {
        let mut state = self.state.write();
        let SubscriptionState {
            connections,
            exact,
            patterns,
        } = &mut *state;
        let Some(info) = connections.get_mut(&conn_id) else {
            return Vec::new();
        };
        let names = if message_types.is_empty() {
            for pattern in info.subscribed_patterns.drain() {
                remove_index(patterns, &pattern, conn_id);
                self.stats
                    .subscriptions_removed
                    .fetch_add(1, Ordering::Relaxed);
            }
            info.subscribed_types.iter().cloned().collect::<Vec<_>>()
        } else {
            message_types.to_vec()
        };
        for name in names {
            if info.subscribed_types.remove(&name) {
                remove_index(exact, &name, conn_id);
                self.stats
                    .subscriptions_removed
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.routes.write().clear();
        let subscriptions = info.subscribed_types.iter().cloned().collect();
        drop(state);
        subscriptions
    }

    /// Atomically adds validated selectors and returns all current patterns sorted.
    ///
    /// Each request and each connection may contain at most 128 patterns.
    /// Selectors match future IPC names automatically. Overlapping selectors and
    /// exact subscriptions deliver only one notification per broadcast.
    ///
    /// # Errors
    /// Returns an error for invalid selectors, exceeded limits, or unknown connections.
    /// No subscriptions are changed on error.
    pub fn subscribe_patterns(
        &self,
        conn_id: ConnectionId,
        values: &[String],
    ) -> Result<Vec<String>, PatternSubscriptionError> {
        let validated = validate_patterns(values)?;
        let mut state = self.state.write();
        let SubscriptionState {
            connections,
            patterns,
            ..
        } = &mut *state;
        let info = connections
            .get_mut(&conn_id)
            .ok_or(PatternSubscriptionError::UnknownConnection)?;
        if info.subscribed_patterns.union(&validated).count() > 128 {
            return Err(PatternSubscriptionError::TooManyPatterns);
        }
        for pattern in validated {
            if info.subscribed_patterns.insert(pattern.clone()) {
                patterns.entry(pattern).or_default().insert(conn_id);
                self.stats
                    .subscriptions_added
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.routes.write().clear();
        let subscriptions = sorted_patterns(&info.subscribed_patterns);
        drop(state);
        Ok(subscriptions)
    }

    /// Atomically removes the specified pattern selectors, returning remaining patterns sorted.
    /// An empty batch removes all patterns while preserving exact subscriptions.
    ///
    /// # Errors
    /// Returns an error for invalid selectors, exceeded request limits, or unknown connections.
    /// No subscriptions are changed on error.
    pub fn unsubscribe_patterns(
        &self,
        conn_id: ConnectionId,
        values: &[String],
    ) -> Result<Vec<String>, PatternSubscriptionError> {
        let validated = validate_patterns(values)?;
        let mut state = self.state.write();
        let SubscriptionState {
            connections,
            patterns,
            ..
        } = &mut *state;
        let info = connections
            .get_mut(&conn_id)
            .ok_or(PatternSubscriptionError::UnknownConnection)?;
        let selected = if values.is_empty() {
            info.subscribed_patterns.clone()
        } else {
            validated
        };
        for pattern in selected {
            if info.subscribed_patterns.remove(&pattern) {
                remove_index(patterns, &pattern, conn_id);
                self.stats
                    .subscriptions_removed
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
        self.routes.write().clear();
        let subscriptions = sorted_patterns(&info.subscribed_patterns);
        drop(state);
        Ok(subscriptions)
    }

    /// Returns the connection's exact subscriptions.
    #[must_use]
    pub fn get_subscriptions(&self, conn_id: ConnectionId) -> Vec<String> {
        self.state
            .read()
            .connections
            .get(&conn_id)
            .map(|info| info.subscribed_types.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Returns the connection's pattern selectors in sorted order.
    #[must_use]
    pub fn get_pattern_subscriptions(&self, conn_id: ConnectionId) -> Vec<String> {
        self.state
            .read()
            .connections
            .get(&conn_id)
            .map(|info| sorted_patterns(&info.subscribed_patterns))
            .unwrap_or_default()
    }

    /// Returns whether a connection has any exact or pattern subscriptions.
    #[must_use]
    pub fn has_subscriptions(&self, conn_id: ConnectionId) -> bool {
        self.state
            .read()
            .connections
            .get(&conn_id)
            .is_some_and(|info| {
                !info.subscribed_types.is_empty() || !info.subscribed_patterns.is_empty()
            })
    }

    /// Registers the IPC name used for an internal broker message type.
    pub fn register_type_mapping(&self, type_id: TypeId, type_name: String) {
        self.type_id_to_name.write().insert(type_id, type_name);
    }

    /// Returns the IPC name for an internal message type.
    #[must_use]
    pub fn get_type_name(&self, type_id: &TypeId) -> Option<String> {
        self.type_id_to_name.read().get(type_id).cloned()
    }

    fn recipients(&self, state: &SubscriptionState, name: &str) -> Arc<[ConnectionId]> {
        if let Some(ids) = self.routes.read().get(name) {
            return Arc::clone(ids);
        }
        let mut ids = state.exact.get(name).cloned().unwrap_or_default();
        for (pattern, subscribers) in &state.patterns {
            if pattern.matches(name) {
                ids.extend(subscribers);
            }
        }
        let ids: Arc<[ConnectionId]> = ids.into_iter().collect();
        if name.len() <= 256 {
            let mut routes = self.routes.write();
            if routes.len() >= 1024 && !routes.contains_key(name) {
                if let Some(evicted) = routes.keys().next().cloned() {
                    routes.remove(&evicted);
                }
            }
            routes.insert(name.to_owned(), Arc::clone(&ids));
        }
        ids
    }

    /// Forwards once to each connection matching an exact name or pattern.
    ///
    /// Recipient sets are memoized for up to 1024 names of at most 256 bytes.
    /// Authorization decisions are never cached: authenticated connections pass
    /// `Deliver` authorization on every notification, outside all manager locks.
    /// Before enqueueing, the manager rechecks the connection incarnation,
    /// cancellation, and current subscriptions. Authorization denials and full or
    /// closed queues count as dropped notifications; stale candidates are skipped.
    /// A concurrent policy change does not retract a completed authorization;
    /// use [`revoke_connection`](Self::revoke_connection) for a synchronized stop.
    pub fn forward_to_subscribers(&self, notification: &IpcPushNotification) {
        let state = self.state.read();
        let ids = self.recipients(&state, &notification.message_type);
        let mut candidates = Vec::new();
        for &conn_id in ids.iter() {
            if let Some(info) = state.connections.get(&conn_id) {
                if info.cancellation.is_cancelled() {
                    continue;
                }
                if let Some((context, policy)) = &info.security {
                    candidates.push(DeliveryCandidate {
                        conn_id,
                        incarnation: Arc::clone(&info.incarnation),
                        context: context.clone(),
                        policy: Arc::clone(policy),
                    });
                } else {
                    self.send_notification(conn_id, &info.push_sender, notification);
                }
            }
        }
        drop(state);

        for candidate in candidates {
            if candidate
                .policy
                .authorize(&candidate.context, IpcOperation::Deliver(notification))
                .is_err()
            {
                self.stats
                    .push_notifications_dropped
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let state = self.state.read();
            let route_unchanged = self
                .routes
                .read()
                .get(&notification.message_type)
                .is_some_and(|current| Arc::ptr_eq(current, &ids));
            if let Some(info) = state.connections.get(&candidate.conn_id) {
                if Arc::ptr_eq(&info.incarnation, &candidate.incarnation)
                    && info
                        .security
                        .as_ref()
                        .is_some_and(|(context, _)| context.same_session(&candidate.context))
                    && !info.cancellation.is_cancelled()
                    && (route_unchanged || info.matches(&notification.message_type))
                {
                    self.send_notification(candidate.conn_id, &info.push_sender, notification);
                }
            }
            drop(state);
        }
    }

    fn send_notification(
        &self,
        conn_id: ConnectionId,
        sender: &PushSender,
        notification: &IpcPushNotification,
    ) {
        match sender.try_send(notification.clone()) {
            Ok(()) => {
                self.stats
                    .push_notifications_sent
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.stats
                    .push_notifications_dropped
                    .fetch_add(1, Ordering::Relaxed);
                warn!(conn_id, message_type = %notification.message_type, "Push channel full, dropping notification");
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.stats
                    .push_notifications_dropped
                    .fetch_add(1, Ordering::Relaxed);
                trace!(conn_id, "Push channel closed");
            }
        }
    }

    /// Returns the number of registered connections.
    #[must_use]
    pub fn connection_count(&self) -> usize {
        self.state.read().connections.len()
    }

    /// Returns the number of distinct exact names with active subscriptions.
    #[must_use]
    pub fn subscribed_types_count(&self) -> usize {
        self.state.read().exact.len()
    }

    /// Returns the number of distinct active pattern selectors.
    #[must_use]
    pub fn subscribed_patterns_count(&self) -> usize {
        self.state.read().patterns.len()
    }

    /// Returns the total number of exact and pattern subscriptions across connections.
    #[must_use]
    pub fn total_subscriptions(&self) -> usize {
        let state = self.state.read();
        state.exact.values().map(HashSet::len).sum::<usize>()
            + state.patterns.values().map(HashSet::len).sum::<usize>()
    }
}

fn sorted_patterns(patterns: &HashSet<SubscriptionPattern>) -> Vec<String> {
    let mut values: Vec<_> = patterns.iter().map(ToString::to_string).collect();
    values.sort();
    values
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

    static_assertions::assert_impl_all!(SubscriptionManager: std::panic::UnwindSafe);

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

    fn notification(name: &str) -> IpcPushNotification {
        IpcPushNotification::new(name, None, serde_json::json!({}))
    }

    #[test]
    fn active_subscriptions_are_scoped_to_each_connection() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);
        manager.register_connection(1, sender, None);
        let (sender, _other_receiver) = mpsc::channel(10);
        manager.register_connection(2, sender, None);
        manager.subscribe_patterns(1, &["*".into()]).unwrap();
        assert!(manager.has_subscriptions(1));
        assert!(!manager.has_subscriptions(2));
        assert!(!manager.has_subscriptions(99));
        manager.subscribe(2, &["Exact".into()]);
        assert!(manager.has_subscriptions(2));
        manager.unsubscribe_patterns(1, &[]).unwrap();
        assert!(!manager.has_subscriptions(1));
        assert!(manager.has_subscriptions(2));
    }

    #[test]
    fn patterns_validate_literal_prefixes_and_utf8_limits() {
        for invalid in ["", "Order", "*Order", "Order**", "Or*der*"] {
            assert_eq!(
                invalid.parse::<SubscriptionPattern>(),
                Err(PatternSubscriptionError::InvalidPattern)
            );
        }
        let pattern: SubscriptionPattern = "注文::*".parse().unwrap();
        assert!(pattern.matches("注文::Created"));
        assert!(pattern.matches("注文::"));
        assert!(!pattern.matches("注文Created"));
        assert!("*".parse::<SubscriptionPattern>().unwrap().matches(""));
        assert!(!"Order*"
            .parse::<SubscriptionPattern>()
            .unwrap()
            .matches("orderCreated"));
        assert!("[?]*"
            .parse::<SubscriptionPattern>()
            .unwrap()
            .matches("[?]Created"));
        assert!(format!("{}*", "a".repeat(255))
            .parse::<SubscriptionPattern>()
            .is_ok());
        assert_eq!(
            format!("{}*", "é".repeat(128)).parse::<SubscriptionPattern>(),
            Err(PatternSubscriptionError::PatternTooLong)
        );
    }

    #[test]
    fn overlapping_selectors_deliver_once_and_reuse_cached_routes() {
        let manager = SubscriptionManager::new();
        let (sender, mut receiver) = mpsc::channel(10);
        manager.register_connection(1, sender, None);
        manager.subscribe(1, &["OrderCreated".into()]);
        assert_eq!(
            manager
                .subscribe_patterns(1, &["Order*".into(), "*".into(), "Order*".into()])
                .unwrap(),
            vec!["*", "Order*"]
        );
        let event = notification("OrderCreated");
        manager.forward_to_subscribers(&event);
        let cached = Arc::clone(manager.routes.read().get("OrderCreated").unwrap());
        manager.forward_to_subscribers(&event);
        assert!(Arc::ptr_eq(
            &cached,
            manager.routes.read().get("OrderCreated").unwrap()
        ));
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
        assert_eq!(manager.total_subscriptions(), 3);
        assert_eq!(manager.subscribed_types_count(), 1);
        assert_eq!(manager.subscribed_patterns_count(), 2);
        assert_eq!(manager.stats().subscriptions_added(), 3);
    }

    #[test]
    fn failed_batches_do_not_change_subscriptions_or_cache() {
        let manager = SubscriptionManager::new();
        let (sender, _receiver) = mpsc::channel(10);
        manager.register_connection(1, sender, None);
        manager.subscribe_patterns(1, &["Keep*".into()]).unwrap();
        manager.forward_to_subscribers(&notification("KeepAlive"));
        let cached = Arc::clone(manager.routes.read().get("KeepAlive").unwrap());
        for values in [
            vec!["Valid*".into(), "invalid".into()],
            vec!["*".into(); 129],
        ] {
            assert!(manager.subscribe_patterns(1, &values).is_err());
            assert!(manager.unsubscribe_patterns(1, &values).is_err());
        }
        assert_eq!(manager.get_pattern_subscriptions(1), vec!["Keep*"]);
        assert!(Arc::ptr_eq(
            &cached,
            manager.routes.read().get("KeepAlive").unwrap()
        ));
        let full: Vec<_> = (0..127).map(|i| format!("{i}*")).collect();
        manager.subscribe_patterns(1, &full).unwrap();
        assert_eq!(
            manager.subscribe_patterns(1, &["Extra*".into()]),
            Err(PatternSubscriptionError::TooManyPatterns)
        );
        assert_eq!(manager.get_pattern_subscriptions(1).len(), 128);
        assert_eq!(
            manager.subscribe_patterns(9, &["*".into()]),
            Err(PatternSubscriptionError::UnknownConnection)
        );
    }

    #[test]
    fn negative_routes_invalidate_and_patterns_match_future_names() {
        let manager = SubscriptionManager::new();
        let (sender, mut receiver) = mpsc::channel(10);
        manager.register_connection(1, sender, None);
        manager.forward_to_subscribers(&notification("OrderCreated"));
        assert!(manager
            .routes
            .read()
            .get("OrderCreated")
            .unwrap()
            .is_empty());
        manager.subscribe_patterns(1, &["Order*".into()]).unwrap();
        assert!(manager.routes.read().is_empty());
        manager.forward_to_subscribers(&notification("OrderCreated"));
        manager.forward_to_subscribers(&notification("OrderFuture"));
        assert_eq!(receiver.try_recv().unwrap().message_type, "OrderCreated");
        assert_eq!(receiver.try_recv().unwrap().message_type, "OrderFuture");
        manager.unsubscribe_patterns(1, &["Order*".into()]).unwrap();
        manager.forward_to_subscribers(&notification("OrderCreated"));
        assert!(receiver.try_recv().is_err());
        manager.subscribe(1, &["OrderCreated".into()]);
        manager.forward_to_subscribers(&notification("OrderCreated"));
        assert!(receiver.try_recv().is_ok());
    }

    #[test]
    fn unsubscribe_semantics_preserve_other_selectors() {
        let manager = SubscriptionManager::new();
        let (sender, mut receiver) = mpsc::channel(10);
        manager.register_connection(1, sender, None);
        manager.subscribe(1, &["OrderCreated".into(), "Literal*".into()]);
        manager.subscribe_patterns(1, &["Order*".into()]).unwrap();
        manager.unsubscribe(1, &["OrderCreated".into()]);
        manager.forward_to_subscribers(&notification("OrderCreated"));
        assert!(receiver.try_recv().is_ok());
        manager.unsubscribe_patterns(1, &[]).unwrap();
        manager.forward_to_subscribers(&notification("LiteralOther"));
        assert!(receiver.try_recv().is_err());
        manager.forward_to_subscribers(&notification("Literal*"));
        assert!(receiver.try_recv().is_ok());
        manager.subscribe_patterns(1, &["*".into()]).unwrap();
        manager.unsubscribe(1, &[]);
        assert!(!manager.has_subscriptions(1));
        assert_eq!(manager.total_subscriptions(), 0);
        assert_eq!(
            manager.stats().subscriptions_added(),
            manager.stats().subscriptions_removed()
        );
    }

    #[test]
    fn replacement_and_disconnect_remove_indices_and_cached_recipients() {
        let manager = SubscriptionManager::new();
        let (sender, mut old_receiver) = mpsc::channel(10);
        manager.register_connection(1, sender, None);
        manager.subscribe(1, &["OrderCreated".into()]);
        manager.subscribe_patterns(1, &["*".into()]).unwrap();
        manager.forward_to_subscribers(&notification("OrderCreated"));
        old_receiver.try_recv().unwrap();
        let (sender, mut receiver) = mpsc::channel(10);
        manager.register_connection(1, sender, None);
        manager.forward_to_subscribers(&notification("OrderCreated"));
        assert!(receiver.try_recv().is_err());
        assert!(!manager.has_subscriptions(1));
        assert_eq!(manager.stats().subscriptions_removed(), 2);
        manager.subscribe_patterns(1, &["*".into()]).unwrap();
        manager.forward_to_subscribers(&notification("OrderCreated"));
        receiver.try_recv().unwrap();
        manager.unregister_connection(1);
        assert!(manager.routes.read().is_empty());
        assert!(!manager.has_subscriptions(1));
        assert_eq!(manager.stats().subscriptions_removed(), 3);
    }

    #[test]
    fn route_cache_is_bounded_and_long_names_still_deliver() {
        let manager = SubscriptionManager::new();
        let (sender, mut receiver) = mpsc::channel(1);
        manager.register_connection(1, sender, None);
        manager.subscribe_patterns(1, &["*".into()]).unwrap();
        for i in 0..1025 {
            manager.forward_to_subscribers(&notification(&format!("Event{i}")));
            receiver.try_recv().unwrap();
            assert!(manager.routes.read().len() <= 1024);
        }
        assert_eq!(manager.routes.read().len(), 1024);
        let long_name = "a".repeat(257);
        manager.forward_to_subscribers(&notification(&long_name));
        assert_eq!(receiver.try_recv().unwrap().message_type, long_name);
        assert!(!manager.routes.read().contains_key(&long_name));
    }

    #[test]
    fn overlapping_patterns_count_drops_once_per_recipient() {
        let manager = SubscriptionManager::new();
        let (sender, receiver) = mpsc::channel(1);
        manager.register_connection(1, sender, None);
        manager
            .subscribe_patterns(1, &["*".into(), "Order*".into()])
            .unwrap();
        let event = notification("OrderCreated");
        manager.forward_to_subscribers(&event);
        manager.forward_to_subscribers(&event);
        assert_eq!(manager.stats().push_notifications_sent(), 1);
        assert_eq!(manager.stats().push_notifications_dropped(), 1);
        drop(receiver);
        manager.forward_to_subscribers(&event);
        assert_eq!(manager.stats().push_notifications_dropped(), 2);
    }

    #[test]
    fn concurrent_mutations_leave_no_stale_routes() {
        let manager = SubscriptionManager::new();
        let (sender, mut receiver) = mpsc::channel(1024);
        manager.register_connection(1, sender, None);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..128 {
                    manager.subscribe_patterns(1, &["*".into()]).unwrap();
                    manager.unsubscribe_patterns(1, &[]).unwrap();
                }
            });
            scope.spawn(|| {
                for _ in 0..128 {
                    manager.subscribe(1, &["Event".into()]);
                    manager.unsubscribe(1, &["Event".into()]);
                }
            });
            for _ in 0..128 {
                manager.forward_to_subscribers(&notification("Event"));
            }
        });
        while receiver.try_recv().is_ok() {}
        manager.forward_to_subscribers(&notification("Event"));
        assert!(receiver.try_recv().is_err());
        assert!(!manager.has_subscriptions(1));
        assert_eq!(
            manager.stats().subscriptions_added(),
            manager.stats().subscriptions_removed()
        );
    }

    struct DeliveryPolicy<F>(F);

    impl<F> IpcSecurityPolicy for DeliveryPolicy<F>
    where
        F: Fn(&IpcConnectionContext) -> Result<(), super::super::security::IpcAccessDenied>
            + Send
            + Sync
            + std::panic::RefUnwindSafe
            + 'static,
    {
        fn admit(
            &self,
            _connection: super::super::security::IpcConnectionInfo,
        ) -> super::super::security::IpcAdmission<'_> {
            Box::pin(async { Ok(super::super::security::IpcIdentity::new(())) })
        }

        fn authorize(
            &self,
            context: &IpcConnectionContext,
            operation: IpcOperation<'_>,
        ) -> Result<(), super::super::security::IpcAccessDenied> {
            assert!(matches!(operation, IpcOperation::Deliver(_)));
            (self.0)(context)
        }
    }

    fn security_context(conn_id: ConnectionId) -> IpcConnectionContext {
        use super::super::security::{IpcConnectionInfo, IpcIdentity};
        IpcConnectionContext::new(
            IpcConnectionInfo::new(conn_id, None),
            IpcIdentity::new("test identity"),
            CancellationToken::new(),
        )
    }

    #[test]
    fn cached_routes_never_cache_authorization_and_overlap_still_delivers_once() {
        use std::sync::atomic::AtomicBool;
        let manager = SubscriptionManager::new();
        let (sender, mut receiver) = mpsc::channel(10);
        let allowed = Arc::new(AtomicBool::new(true));
        let calls = Arc::new(AtomicUsize::new(0));
        let policy_allowed = Arc::clone(&allowed);
        let policy_calls = Arc::clone(&calls);
        let policy = DeliveryPolicy(move |context: &IpcConnectionContext| {
            assert_eq!(context.identity::<&str>(), Some(&"test identity"));
            policy_calls.fetch_add(1, Ordering::SeqCst);
            if policy_allowed.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err(super::super::security::IpcAccessDenied::new(
                    "permission changed",
                ))
            }
        });
        manager.register_authenticated_connection(1, sender, security_context(1), Arc::new(policy));
        manager.subscribe(1, &["OrderCreated".into()]);
        manager
            .subscribe_patterns(1, &["*".into(), "Order*".into()])
            .unwrap();
        let event = notification("OrderCreated");
        manager.forward_to_subscribers(&event);
        receiver.try_recv().unwrap();
        let cached = Arc::clone(manager.routes.read().get("OrderCreated").unwrap());
        allowed.store(false, Ordering::SeqCst);
        manager.forward_to_subscribers(&event);
        assert!(receiver.try_recv().is_err());
        allowed.store(true, Ordering::SeqCst);
        manager.forward_to_subscribers(&event);
        receiver.try_recv().unwrap();
        assert!(receiver.try_recv().is_err());
        assert!(Arc::ptr_eq(
            &cached,
            manager.routes.read().get("OrderCreated").unwrap()
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!(manager.stats().push_notifications_sent(), 2);
        assert_eq!(manager.stats().push_notifications_dropped(), 1);
    }

    #[test]
    fn mutations_during_authorization_cannot_deliver_stale_candidates() {
        use std::sync::Barrier;
        for mutation in [
            "revoke",
            "disconnect",
            "replace",
            "unsubscribe",
            "unsubscribe_patterns",
        ] {
            let manager = Arc::new(SubscriptionManager::new());
            let entered = Arc::new(Barrier::new(2));
            let resume = Arc::new(Barrier::new(2));
            let policy_entered = Arc::clone(&entered);
            let policy_resume = Arc::clone(&resume);
            let policy = DeliveryPolicy(move |_context: &IpcConnectionContext| {
                policy_entered.wait();
                policy_resume.wait();
                Ok(())
            });
            let (sender, mut receiver) = mpsc::channel(10);
            let context = security_context(1);
            manager.register_authenticated_connection(1, sender, context.clone(), Arc::new(policy));
            if mutation == "unsubscribe_patterns" {
                manager.subscribe_patterns(1, &["Event*".into()]).unwrap();
            } else {
                manager.subscribe(1, &["Event".into()]);
            }
            let (replacement_sender, mut replacement_receiver) = mpsc::channel(10);
            std::thread::scope(|scope| {
                scope.spawn(|| manager.forward_to_subscribers(&notification("Event")));
                entered.wait();
                match mutation {
                    "revoke" => {
                        assert!(manager.revoke_connection(1));
                    }
                    "disconnect" => manager.unregister_connection(1),
                    "replace" => {
                        manager.register_connection(1, replacement_sender, None);
                        manager.subscribe(1, &["Event".into()]);
                    }
                    "unsubscribe" => {
                        manager.unsubscribe(1, &["Event".into()]);
                    }
                    "unsubscribe_patterns" => {
                        manager.unsubscribe_patterns(1, &[]).unwrap();
                    }
                    _ => unreachable!(),
                }
                resume.wait();
            });
            assert!(receiver.try_recv().is_err(), "{mutation}");
            assert!(replacement_receiver.try_recv().is_err(), "{mutation}");
            assert_eq!(manager.stats().push_notifications_sent(), 0);
            assert_eq!(
                context.is_revoked(),
                matches!(mutation, "revoke" | "disconnect" | "replace")
            );
        }
    }

    #[test]
    fn policy_can_reenter_manager_and_remove_its_own_subscription() {
        let manager = Arc::new(SubscriptionManager::new());
        let weak_manager = std::sync::Mutex::new(Arc::downgrade(&manager));
        let policy = DeliveryPolicy(move |context: &IpcConnectionContext| {
            let manager = weak_manager.lock().unwrap().upgrade().unwrap();
            assert!(manager
                .connection_context(context.connection_id())
                .unwrap()
                .same_session(context));
            manager.unsubscribe(context.connection_id(), &[]);
            Ok(())
        });
        let (sender, mut receiver) = mpsc::channel(10);
        manager.register_authenticated_connection(1, sender, security_context(1), Arc::new(policy));
        manager.subscribe_patterns(1, &["*".into()]).unwrap();
        manager.forward_to_subscribers(&notification("Event"));
        assert!(receiver.try_recv().is_err());
        assert!(!manager.has_subscriptions(1));
    }

    #[test]
    fn default_connections_can_be_revoked_and_cancel_their_transport_token() {
        let manager = SubscriptionManager::new();
        let token = CancellationToken::new();
        let (sender, mut receiver) = mpsc::channel(10);
        manager.register_connection_with_token(1, sender, None, token.clone());
        manager.subscribe(1, &["Event".into()]);
        manager.subscribe_patterns(1, &["*".into()]).unwrap();
        assert!(manager.connection_context(1).is_none());
        manager.forward_to_subscribers(&notification("Event"));
        receiver.try_recv().unwrap();
        assert!(manager.revoke_connection(1));
        assert!(!manager.revoke_connection(1));
        assert!(token.is_cancelled());
        assert_eq!(manager.connection_count(), 0);
        assert_eq!(manager.total_subscriptions(), 0);
        assert_eq!(manager.stats().subscriptions_removed(), 2);
        assert!(manager.routes.read().is_empty());
        manager.forward_to_subscribers(&notification("Event"));
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn replacement_and_unregister_cancel_only_the_previous_session() {
        let manager = SubscriptionManager::new();
        let old = security_context(1);
        let (sender, _receiver) = mpsc::channel(10);
        manager.register_authenticated_connection(
            1,
            sender,
            old.clone(),
            Arc::new(DeliveryPolicy(|_: &IpcConnectionContext| Ok(()))),
        );
        let retained = manager.connection_context(1).unwrap();
        assert!(retained.same_session(&old));
        let fresh = security_context(1);
        let (sender, _receiver) = mpsc::channel(10);
        manager.register_authenticated_connection(
            1,
            sender,
            fresh.clone(),
            Arc::new(DeliveryPolicy(|_: &IpcConnectionContext| Ok(()))),
        );
        assert!(old.is_revoked());
        assert!(retained.is_revoked());
        assert!(!fresh.is_revoked());
        assert!(manager.connection_context(1).unwrap().same_session(&fresh));
        manager.unregister_connection(1);
        assert!(fresh.is_revoked());
        assert!(manager.connection_context(1).is_none());
    }
    #[test]
    fn unrelated_subscription_changes_during_authorization_preserve_valid_delivery() {
        let manager = Arc::new(SubscriptionManager::new());
        let weak_manager = std::sync::Mutex::new(Arc::downgrade(&manager));
        let policy = DeliveryPolicy(move |_context: &IpcConnectionContext| {
            let manager = weak_manager.lock().unwrap().upgrade().unwrap();
            manager.subscribe(2, &["Unrelated".into()]);
            Ok(())
        });
        let (sender, mut receiver) = mpsc::channel(10);
        manager.register_authenticated_connection(1, sender, security_context(1), Arc::new(policy));
        let (sender, _other_receiver) = mpsc::channel(10);
        manager.register_connection(2, sender, None);
        manager.subscribe_patterns(1, &["Event*".into()]).unwrap();
        manager.forward_to_subscribers(&notification("EventCreated"));
        assert_eq!(receiver.try_recv().unwrap().message_type, "EventCreated");
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn removal_drops_application_identity_and_policy_outside_manager_locks() {
        use super::super::security::{IpcConnectionInfo, IpcIdentity};
        use std::sync::{Mutex, Weak};

        struct DropObserver {
            manager: Mutex<Weak<SubscriptionManager>>,
            drops: Arc<AtomicUsize>,
        }

        impl DropObserver {
            fn observe(&self) {
                assert_eq!(self.drops.load(Ordering::SeqCst), 0);
            }
        }

        impl Drop for DropObserver {
            fn drop(&mut self) {
                let manager = self.manager.lock().unwrap().upgrade();
                if let Some(manager) = manager {
                    manager.unsubscribe(1, &[]);
                    self.drops.fetch_add(1, Ordering::SeqCst);
                }
            }
        }

        for removal in ["replace", "unregister", "revoke"] {
            let manager = Arc::new(SubscriptionManager::new());
            let drops = Arc::new(AtomicUsize::new(0));
            let identity = IpcIdentity::new(DropObserver {
                manager: Mutex::new(Arc::downgrade(&manager)),
                drops: Arc::clone(&drops),
            });
            let context = IpcConnectionContext::new(
                IpcConnectionInfo::new(1, None),
                identity,
                CancellationToken::new(),
            );
            let observer = DropObserver {
                manager: Mutex::new(Arc::downgrade(&manager)),
                drops: Arc::clone(&drops),
            };
            let policy = DeliveryPolicy(move |_context: &IpcConnectionContext| {
                observer.observe();
                Ok(())
            });
            let (sender, _receiver) = mpsc::channel(10);
            manager.register_authenticated_connection(1, sender, context, Arc::new(policy));
            manager.subscribe_patterns(1, &["*".into()]).unwrap();
            match removal {
                "replace" => {
                    let (sender, _receiver) = mpsc::channel(10);
                    manager.register_connection(1, sender, None);
                }
                "unregister" => manager.unregister_connection(1),
                "revoke" => {
                    assert!(manager.revoke_connection(1));
                }
                _ => unreachable!(),
            }
            assert_eq!(drops.load(Ordering::SeqCst), 2, "{removal}");
            assert_eq!(manager.total_subscriptions(), 0);
            assert_eq!(manager.stats().subscriptions_removed(), 1);
        }
    }
}
