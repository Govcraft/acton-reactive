//! Regression coverage for issue #22: the type returned by the subscription
//! manager must be nameable by applications through the public IPC facade.
#![cfg(feature = "ipc")]

use acton_reactive::ipc::{SubscriptionManager, SubscriptionStats};

struct SubscriptionMetrics<'a> {
    stats: &'a SubscriptionStats,
}

const fn subscription_stats(manager: &SubscriptionManager) -> &SubscriptionStats {
    manager.stats()
}

#[test]
fn subscription_stats_are_nameable_and_borrowable_via_public_path() {
    let manager = SubscriptionManager::new();
    let metrics = SubscriptionMetrics {
        stats: subscription_stats(&manager),
    };

    assert!(std::ptr::eq(metrics.stats, manager.stats()));
    assert_eq!(metrics.stats.subscriptions_added(), 0);
    assert_eq!(metrics.stats.subscriptions_removed(), 0);
    assert_eq!(metrics.stats.push_notifications_sent(), 0);
    assert_eq!(metrics.stats.push_notifications_dropped(), 0);
}
