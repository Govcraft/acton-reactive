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

//! Tests for examples from docs/pub-sub/page.md
//!
//! This module verifies that the code examples from the "Pub/Sub Broadcasting"
//! documentation page compile and run correctly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Tests basic broker broadcasting.
///
/// From: docs/pub-sub/page.md - "The Broker"
#[acton_test]
async fn test_broker_basics() -> anyhow::Result<()> {
    #[acton_actor]
    struct Subscriber {
        received: bool,
    }

    #[acton_message]
    struct TestEvent;

    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Get the broker
    let broker = runtime.broker();

    // Create subscriber
    let mut subscriber = runtime.new_actor::<Subscriber>();
    subscriber.mutate_on::<TestEvent>(move |actor, _ctx| {
        actor.model.received = true;
        received_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });

    // Subscribe before starting
    subscriber.handle().subscribe::<TestEvent>().await;
    let _handle = subscriber.start().await;

    // Broadcast
    broker.broadcast(TestEvent).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert!(received.load(Ordering::SeqCst));

    Ok(())
}

/// Tests price feed example with multiple subscribers.
///
/// From: docs/pub-sub/page.md - "Example: Price Feed"
#[acton_test]
async fn test_price_feed_example() -> anyhow::Result<()> {
    #[acton_actor]
    struct PriceDisplay {
        last_price: Option<f64>,
    }

    #[acton_actor]
    struct PriceLogger {
        history: Vec<PriceUpdate>,
    }

    #[acton_message]
    struct PriceUpdate {
        symbol: String,
        price: f64,
    }

    let display_received = Arc::new(AtomicBool::new(false));
    let display_clone = display_received.clone();
    let logger_count = Arc::new(AtomicU32::new(0));
    let logger_clone = logger_count.clone();

    let mut runtime = ActonApp::launch_async().await;

    // Price display actor
    let mut display = runtime.new_actor::<PriceDisplay>();
    display.mutate_on::<PriceUpdate>(move |actor, ctx| {
        let update = ctx.message();
        actor.model.last_price = Some(update.price);
        display_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });
    display.handle().subscribe::<PriceUpdate>().await;
    let _display = display.start().await;

    // Price logger actor
    let mut logger = runtime.new_actor::<PriceLogger>();
    logger
        .mutate_on::<PriceUpdate>(|actor, ctx| {
            actor.model.history.push(ctx.message().clone());
            Reply::ready()
        })
        .after_stop(move |actor| {
            logger_clone.store(actor.model.history.len() as u32, Ordering::SeqCst);
            Reply::ready()
        });
    logger.handle().subscribe::<PriceUpdate>().await;
    let _logger = logger.start().await;

    // Broadcast reaches both
    let broker = runtime.broker();
    broker
        .broadcast(PriceUpdate {
            symbol: "ACME".into(),
            price: 150.0,
        })
        .await;
    broker
        .broadcast(PriceUpdate {
            symbol: "ACME".into(),
            price: 155.0,
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert!(display_received.load(Ordering::SeqCst));
    assert_eq!(logger_count.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Tests unsubscribing from messages.
///
/// From: docs/pub-sub/page.md - "Unsubscribing"
///
/// NOTE: This test is ignored because the unsubscribe feature is not fully
/// implemented in acton-reactive. The UnsubscribeBroker message has all its
/// fields commented out. See: acton-reactive/src/message/unsubscribe_broker.rs
#[acton_test]
#[ignore = "unsubscribe feature not fully implemented"]
async fn test_unsubscribing() -> anyhow::Result<()> {
    #[acton_actor]
    struct Subscriber {
        receive_count: u32,
    }

    #[acton_message]
    struct Event;

    let final_count = Arc::new(AtomicU32::new(0));
    let final_clone = final_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let mut subscriber = runtime.new_actor::<Subscriber>();
    subscriber
        .mutate_on::<Event>(|actor, _ctx| {
            actor.model.receive_count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            final_clone.store(actor.model.receive_count, Ordering::SeqCst);
            Reply::ready()
        });

    subscriber.handle().subscribe::<Event>().await;
    let handle = subscriber.start().await;

    // First broadcast - should receive
    broker.broadcast(Event).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Unsubscribe
    handle.unsubscribe::<Event>();

    // Second broadcast - should NOT receive
    broker.broadcast(Event).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    runtime.shutdown_all().await?;

    // Should only have received 1 event
    assert_eq!(final_count.load(Ordering::SeqCst), 1);

    Ok(())
}

/// Tests subscribing to multiple message types.
///
/// From: docs/pub-sub/page.md - "Multiple Message Types"
#[acton_test]
async fn test_multiple_message_types() -> anyhow::Result<()> {
    #[acton_actor]
    struct MultiSubscriber {
        price_updates: u32,
        volume_updates: u32,
        trade_events: u32,
    }

    #[acton_message]
    struct PriceUpdate;

    #[acton_message]
    struct VolumeUpdate;

    #[acton_message]
    struct TradeExecuted;

    let final_counts = Arc::new(std::sync::Mutex::new((0u32, 0u32, 0u32)));
    let final_clone = final_counts.clone();

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let mut subscriber = runtime.new_actor::<MultiSubscriber>();

    subscriber
        .mutate_on::<PriceUpdate>(|actor, _ctx| {
            actor.model.price_updates += 1;
            Reply::ready()
        })
        .mutate_on::<VolumeUpdate>(|actor, _ctx| {
            actor.model.volume_updates += 1;
            Reply::ready()
        })
        .mutate_on::<TradeExecuted>(|actor, _ctx| {
            actor.model.trade_events += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            *final_clone.lock().unwrap() = (
                actor.model.price_updates,
                actor.model.volume_updates,
                actor.model.trade_events,
            );
            Reply::ready()
        });

    // Subscribe to multiple types
    let handle = subscriber.handle().clone();
    handle.subscribe::<PriceUpdate>().await;
    handle.subscribe::<VolumeUpdate>().await;
    handle.subscribe::<TradeExecuted>().await;

    let _handle = subscriber.start().await;

    // Broadcast different types
    broker.broadcast(PriceUpdate).await;
    broker.broadcast(PriceUpdate).await;
    broker.broadcast(VolumeUpdate).await;
    broker.broadcast(TradeExecuted).await;
    broker.broadcast(TradeExecuted).await;
    broker.broadcast(TradeExecuted).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    let counts = final_counts.lock().unwrap();
    assert_eq!(counts.0, 2); // price updates
    assert_eq!(counts.1, 1); // volume updates
    assert_eq!(counts.2, 3); // trade events

    Ok(())
}

/// Tests filtering broadcasts in handler.
///
/// From: docs/pub-sub/page.md - "Filtering Broadcasts"
#[acton_test]
async fn test_filtering_broadcasts() -> anyhow::Result<()> {
    use std::collections::HashMap;
    use std::collections::HashSet;

    #[acton_actor]
    struct FilteredSubscriber {
        watched_symbols: HashSet<String>,
        prices: HashMap<String, f64>,
    }

    #[acton_message]
    struct PriceUpdate {
        symbol: String,
        price: f64,
    }

    let final_prices = Arc::new(std::sync::Mutex::new(HashMap::<String, f64>::new()));
    let final_clone = final_prices.clone();

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let mut subscriber = runtime.new_actor::<FilteredSubscriber>();
    subscriber
        .model
        .watched_symbols
        .insert("ACME".to_string());
    subscriber
        .model
        .watched_symbols
        .insert("TECH".to_string());

    subscriber
        .mutate_on::<PriceUpdate>(|actor, ctx| {
            let update = ctx.message();

            // Only care about specific symbols
            if !actor.model.watched_symbols.contains(&update.symbol) {
                return Reply::ready();
            }

            // Process the update
            actor
                .model
                .prices
                .insert(update.symbol.clone(), update.price);
            Reply::ready()
        })
        .after_stop(move |actor| {
            *final_clone.lock().unwrap() = actor.model.prices.clone();
            Reply::ready()
        });

    subscriber.handle().subscribe::<PriceUpdate>().await;
    let _handle = subscriber.start().await;

    // Broadcast various symbols
    broker
        .broadcast(PriceUpdate {
            symbol: "ACME".into(),
            price: 100.0,
        })
        .await;
    broker
        .broadcast(PriceUpdate {
            symbol: "OTHER".into(),
            price: 50.0,
        })
        .await; // Should be filtered
    broker
        .broadcast(PriceUpdate {
            symbol: "TECH".into(),
            price: 200.0,
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    let prices = final_prices.lock().unwrap();
    assert_eq!(prices.len(), 2); // Only ACME and TECH
    assert!(prices.contains_key("ACME"));
    assert!(prices.contains_key("TECH"));
    assert!(!prices.contains_key("OTHER"));

    Ok(())
}

/// Tests event bus pattern.
///
/// From: docs/pub-sub/page.md - "Event Bus"
#[acton_test]
async fn test_event_bus_pattern() -> anyhow::Result<()> {
    #[acton_actor]
    struct Analytics {
        login_count: u32,
        order_count: u32,
    }

    #[acton_actor]
    struct Notifications {
        order_notifications: u32,
    }

    #[acton_message]
    struct UserLoggedIn {
        user_id: String,
    }

    #[acton_message]
    struct OrderPlaced {
        order_id: String,
        user_id: String,
    }

    let analytics_counts = Arc::new(std::sync::Mutex::new((0u32, 0u32)));
    let analytics_clone = analytics_counts.clone();
    let notification_count = Arc::new(AtomicU32::new(0));
    let notification_clone = notification_count.clone();

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    // Analytics subscribes to everything
    let mut analytics = runtime.new_actor::<Analytics>();
    analytics
        .mutate_on::<UserLoggedIn>(|actor, _ctx| {
            actor.model.login_count += 1;
            Reply::ready()
        })
        .mutate_on::<OrderPlaced>(|actor, _ctx| {
            actor.model.order_count += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            *analytics_clone.lock().unwrap() =
                (actor.model.login_count, actor.model.order_count);
            Reply::ready()
        });

    analytics.handle().subscribe::<UserLoggedIn>().await;
    analytics.handle().subscribe::<OrderPlaced>().await;
    let _analytics = analytics.start().await;

    // Notification service only cares about orders
    let mut notifications = runtime.new_actor::<Notifications>();
    notifications
        .mutate_on::<OrderPlaced>(|actor, _ctx| {
            actor.model.order_notifications += 1;
            Reply::ready()
        })
        .after_stop(move |actor| {
            notification_clone.store(actor.model.order_notifications, Ordering::SeqCst);
            Reply::ready()
        });

    notifications.handle().subscribe::<OrderPlaced>().await;
    let _notifications = notifications.start().await;

    // Publishers broadcast events
    broker
        .broadcast(UserLoggedIn {
            user_id: "123".into(),
        })
        .await;
    broker
        .broadcast(UserLoggedIn {
            user_id: "456".into(),
        })
        .await;
    broker
        .broadcast(OrderPlaced {
            order_id: "ORD-001".into(),
            user_id: "123".into(),
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    let counts = analytics_counts.lock().unwrap();
    assert_eq!(counts.0, 2); // login count
    assert_eq!(counts.1, 1); // order count
    assert_eq!(notification_count.load(Ordering::SeqCst), 1);

    Ok(())
}

/// Tests system alerts pattern.
///
/// From: docs/pub-sub/page.md - "System Alerts"
#[acton_test]
async fn test_system_alerts_pattern() -> anyhow::Result<()> {
    #[acton_actor]
    struct Worker {
        accepting_new_work: bool,
        in_maintenance: bool,
    }

    #[acton_message]
    enum SystemAlert {
        Shutdown { in_seconds: u32 },
        MaintenanceMode,
        NormalOperation,
    }

    let final_state = Arc::new(std::sync::Mutex::new((true, false)));
    let final_clone = final_state.clone();

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    let mut worker = runtime.new_actor::<Worker>();
    worker.model.accepting_new_work = true;

    worker
        .mutate_on::<SystemAlert>(|actor, ctx| {
            match ctx.message() {
                SystemAlert::Shutdown { in_seconds: _ } => {
                    actor.model.accepting_new_work = false;
                }
                SystemAlert::MaintenanceMode => {
                    actor.model.in_maintenance = true;
                }
                SystemAlert::NormalOperation => {
                    actor.model.in_maintenance = false;
                    actor.model.accepting_new_work = true;
                }
            }
            Reply::ready()
        })
        .after_stop(move |actor| {
            *final_clone.lock().unwrap() =
                (actor.model.accepting_new_work, actor.model.in_maintenance);
            Reply::ready()
        });

    worker.handle().subscribe::<SystemAlert>().await;
    let _worker = worker.start().await;

    // Send maintenance alert
    broker.broadcast(SystemAlert::MaintenanceMode).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    runtime.shutdown_all().await?;

    let state = final_state.lock().unwrap();
    assert!(state.1); // in_maintenance should be true

    Ok(())
}

/// Tests broadcasting from within a handler.
///
/// From: docs/pub-sub/page.md - "From within a handler"
#[acton_test]
async fn test_broadcast_from_handler() -> anyhow::Result<()> {
    #[acton_actor]
    struct Publisher;

    #[acton_actor]
    struct Subscriber {
        received: bool,
    }

    #[acton_message]
    struct PriceChanged {
        symbol: String,
        new_price: f64,
    }

    #[acton_message]
    struct PriceUpdate {
        symbol: String,
        price: f64,
    }

    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();

    let mut runtime = ActonApp::launch_async().await;
    let broker = runtime.broker();

    // Create subscriber first
    let mut subscriber = runtime.new_actor::<Subscriber>();
    subscriber.mutate_on::<PriceUpdate>(move |actor, _ctx| {
        actor.model.received = true;
        received_clone.store(true, Ordering::SeqCst);
        Reply::ready()
    });
    subscriber.handle().subscribe::<PriceUpdate>().await;
    let _subscriber = subscriber.start().await;

    // Create publisher
    let mut publisher = runtime.new_actor::<Publisher>();
    let broker_clone = broker.clone();

    publisher.mutate_on::<PriceChanged>(move |_actor, ctx| {
        let broker = broker_clone.clone();
        let update = PriceUpdate {
            symbol: ctx.message().symbol.clone(),
            price: ctx.message().new_price,
        };

        Reply::pending(async move {
            broker.broadcast(update).await;
        })
    });

    let publisher_handle = publisher.start().await;

    // Trigger broadcast from handler
    publisher_handle
        .send(PriceChanged {
            symbol: "ACME".into(),
            new_price: 150.0,
        })
        .await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    runtime.shutdown_all().await?;

    assert!(received.load(Ordering::SeqCst));

    Ok(())
}
