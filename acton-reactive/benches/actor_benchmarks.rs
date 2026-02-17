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

//! Benchmarks for the acton-reactive actor framework.
//!
//! This benchmark suite measures the performance of core actor operations:
//! - Actor creation and spawning
//! - Message throughput (fire-and-forget)
//! - Request-reply latency (ping-pong)
//! - Broker pub/sub with varying subscriber counts
//!
//! Run with: `cargo bench --package acton-reactive`

use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use acton_reactive::prelude::*;
use divan::{AllocProfiler, Bencher};

// Enable allocation tracking
#[global_allocator]
static ALLOC: AllocProfiler = AllocProfiler::system();

fn main() {
    divan::main();
}

// =============================================================================
// Test Messages
// =============================================================================

/// Simple increment message for throughput benchmarks.
#[acton_message]
struct Increment;

/// Ping message for request-reply latency benchmarks.
#[acton_message]
struct Ping;

/// Pong reply message.
#[acton_message]
struct Pong;

/// Broadcast message for pub/sub benchmarks.
#[acton_message]
struct BroadcastEvent {
    #[allow(dead_code)]
    value: u64,
}

// =============================================================================
// Test Actors
// =============================================================================

/// Simple counter actor for throughput benchmarks.
#[derive(Debug, Default, Clone)]
struct CounterActor {
    #[allow(dead_code)]
    count: usize,
}

/// Actor that responds to Ping with Pong for latency benchmarks.
#[derive(Debug, Default, Clone)]
struct PingPongActor;

/// Actor that receives broadcast messages.
#[derive(Debug, Default, Clone)]
struct SubscriberActor {
    #[allow(dead_code)]
    received: usize,
}

// =============================================================================
// Actor Creation Benchmarks
// =============================================================================

/// Benchmarks the cost of creating a single actor.
#[divan::bench]
fn actor_creation(bencher: Bencher<'_, '_>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            let actor = runtime.new_actor_with_name::<CounterActor>("bench_actor".to_string());
            let handle = actor.start().await;

            black_box(&handle);

            // Clean shutdown
            let _ = handle.stop().await;
            let _ = runtime.shutdown_all().await;
        });
    });
}

/// Benchmarks creating multiple actors in sequence.
#[divan::bench(args = [1, 10, 100])]
fn actor_creation_batch(bencher: Bencher<'_, '_>, count: usize) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            let mut handles = Vec::with_capacity(count);
            for i in 0..count {
                let actor =
                    runtime.new_actor_with_name::<CounterActor>(format!("bench_actor_{i}"));
                handles.push(actor.start().await);
            }

            black_box(&handles);

            // Clean shutdown
            for handle in &handles {
                let _ = handle.stop().await;
            }
            let _ = runtime.shutdown_all().await;
        });
    });
}

// =============================================================================
// Message Throughput Benchmarks
// =============================================================================

/// Benchmarks sending messages to a single actor (fire-and-forget).
#[divan::bench(args = [100, 1000, 10000])]
fn message_throughput_single_actor(bencher: Bencher<'_, '_>, message_count: usize) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            let received = Arc::new(AtomicUsize::new(0));
            let received_clone = received.clone();
            let expected = message_count;

            let mut actor =
                runtime.new_actor_with_name::<CounterActor>("throughput_actor".to_string());

            actor.mutate_on::<Increment>(move |_actor, _event| {
                received_clone.fetch_add(1, Ordering::SeqCst);
                Reply::ready()
            });

            let handle = actor.start().await;

            // Send all messages
            for _ in 0..message_count {
                handle.send(Increment).await;
            }

            // Wait for all messages to be processed
            while received.load(Ordering::SeqCst) < expected {
                tokio::task::yield_now().await;
            }

            black_box(received.load(Ordering::SeqCst));

            // Clean shutdown
            let _ = handle.stop().await;
            let _ = runtime.shutdown_all().await;
        });
    });
}

/// Benchmarks round-robin message distribution across multiple actors.
#[divan::bench(args = [1, 4, 8, 16])]
fn message_throughput_multi_actor(bencher: Bencher<'_, '_>, actor_count: usize) {
    const MESSAGES_PER_ACTOR: usize = 1000;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            let total_messages = actor_count * MESSAGES_PER_ACTOR;
            let received = Arc::new(AtomicUsize::new(0));

            let mut handles = Vec::with_capacity(actor_count);

            for i in 0..actor_count {
                let received_clone = received.clone();
                let mut actor =
                    runtime.new_actor_with_name::<CounterActor>(format!("throughput_actor_{i}"));

                actor.mutate_on::<Increment>(move |_actor, _event| {
                    received_clone.fetch_add(1, Ordering::SeqCst);
                    Reply::ready()
                });

                handles.push(actor.start().await);
            }

            // Round-robin message distribution
            for i in 0..total_messages {
                let handle = &handles[i % actor_count];
                handle.send(Increment).await;
            }

            // Wait for all messages to be processed
            while received.load(Ordering::SeqCst) < total_messages {
                tokio::task::yield_now().await;
            }

            black_box(received.load(Ordering::SeqCst));

            // Clean shutdown
            for handle in &handles {
                let _ = handle.stop().await;
            }
            let _ = runtime.shutdown_all().await;
        });
    });
}

// =============================================================================
// Send-to-Handler Hot Path Benchmarks
// =============================================================================

/// Benchmarks the complete hot path: handle.send(msg) → handler invocation.
///
/// Actor setup happens once outside the measurement loop. Each iteration
/// measures only: envelope creation → channel send → inbox recv → type
/// dispatch → handler invocation.
#[divan::bench]
fn send_to_handler_latency(bencher: Bencher<'_, '_>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let (handle, notify, _runtime) = rt.block_on(async {
        let mut runtime = ActonApp::launch_async().await;
        let notify = Arc::new(tokio::sync::Notify::new());
        let notify_clone = notify.clone();

        let mut actor =
            runtime.new_actor_with_name::<CounterActor>("latency_actor".to_string());

        actor.mutate_on::<Increment>(move |_actor, _event| {
            notify_clone.notify_one();
            Reply::ready()
        });

        let handle = actor.start().await;
        (handle, notify, runtime)
    });

    bencher.bench_local(|| {
        rt.block_on(async {
            handle.send(Increment).await;
            notify.notified().await;
        });
    });

    rt.block_on(async {
        let _ = handle.stop().await;
    });
}

// =============================================================================
// Request-Reply Latency Benchmarks (Ping-Pong)
// =============================================================================

/// Benchmarks request-reply latency with a single ping-pong exchange.
#[divan::bench]
fn ping_pong_single(bencher: Bencher<'_, '_>) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            let pong_received = Arc::new(AtomicUsize::new(0));
            let pong_clone = pong_received.clone();

            // Create responder actor
            let mut responder =
                runtime.new_actor_with_name::<PingPongActor>("responder".to_string());

            responder.act_on::<Ping>(|_actor, event| {
                let reply_envelope = event.reply_envelope();
                Box::pin(async move {
                    reply_envelope.send(Pong).await;
                })
            });

            let responder_handle = responder.start().await;

            // Create requester actor
            let mut requester =
                runtime.new_actor_with_name::<PingPongActor>("requester".to_string());

            requester.act_on::<Pong>(move |_actor, _event| {
                let pong = pong_clone.clone();
                Box::pin(async move {
                    pong.fetch_add(1, Ordering::SeqCst);
                })
            });

            let requester_handle = requester.start().await;

            // Send ping
            let envelope =
                requester_handle.create_envelope(Some(responder_handle.reply_address()));
            envelope.send(Ping).await;

            // Wait for pong
            while pong_received.load(Ordering::SeqCst) < 1 {
                tokio::task::yield_now().await;
            }

            black_box(pong_received.load(Ordering::SeqCst));

            // Clean shutdown
            let _ = requester_handle.stop().await;
            let _ = responder_handle.stop().await;
            let _ = runtime.shutdown_all().await;
        });
    });
}

/// Benchmarks sustained request-reply latency with multiple ping-pong exchanges.
#[divan::bench(args = [10, 100, 1000])]
fn ping_pong_sustained(bencher: Bencher<'_, '_>, exchange_count: usize) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            let pong_received = Arc::new(AtomicUsize::new(0));
            let pong_clone = pong_received.clone();

            // Create responder actor
            let mut responder =
                runtime.new_actor_with_name::<PingPongActor>("responder".to_string());

            responder.act_on::<Ping>(|_actor, event| {
                let reply_envelope = event.reply_envelope();
                Box::pin(async move {
                    reply_envelope.send(Pong).await;
                })
            });

            let responder_handle = responder.start().await;

            // Create requester actor
            let mut requester =
                runtime.new_actor_with_name::<PingPongActor>("requester".to_string());

            requester.act_on::<Pong>(move |_actor, _event| {
                let pong = pong_clone.clone();
                Box::pin(async move {
                    pong.fetch_add(1, Ordering::SeqCst);
                })
            });

            let requester_handle = requester.start().await;

            // Send pings and wait for pongs
            let envelope =
                requester_handle.create_envelope(Some(responder_handle.reply_address()));

            for _ in 0..exchange_count {
                envelope.send(Ping).await;
            }

            // Wait for all pongs
            while pong_received.load(Ordering::SeqCst) < exchange_count {
                tokio::task::yield_now().await;
            }

            black_box(pong_received.load(Ordering::SeqCst));

            // Clean shutdown
            let _ = requester_handle.stop().await;
            let _ = responder_handle.stop().await;
            let _ = runtime.shutdown_all().await;
        });
    });
}

// =============================================================================
// Broker Pub/Sub Benchmarks
// =============================================================================

/// Benchmarks broker broadcast with varying subscriber counts.
#[divan::bench(args = [1, 10, 50, 100])]
fn broker_broadcast(bencher: Bencher<'_, '_>, subscriber_count: usize) {
    const BROADCAST_COUNT: usize = 100;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            let total_expected = subscriber_count * BROADCAST_COUNT;
            let received = Arc::new(AtomicUsize::new(0));

            let mut handles = Vec::with_capacity(subscriber_count);

            // Create subscriber actors
            for i in 0..subscriber_count {
                let received_clone = received.clone();
                let mut actor =
                    runtime.new_actor_with_name::<SubscriberActor>(format!("subscriber_{i}"));

                actor.act_on::<BroadcastEvent>(move |_actor, _event| {
                    let received = received_clone.clone();
                    Box::pin(async move {
                        received.fetch_add(1, Ordering::SeqCst);
                    })
                });

                let handle = actor.start().await;

                // Subscribe to BroadcastEvent
                handle.subscribe::<BroadcastEvent>().await;

                handles.push(handle);
            }

            // Give subscriptions time to register
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

            // Get broker handle from first actor
            let broker = handles[0].get_broker().expect("Broker should be available");

            // Broadcast messages
            for i in 0..BROADCAST_COUNT {
                broker
                    .send(BrokerRequest::new(BroadcastEvent { value: i as u64 }))
                    .await;
            }

            // Wait for all messages to be received
            while received.load(Ordering::SeqCst) < total_expected {
                tokio::task::yield_now().await;
            }

            black_box(received.load(Ordering::SeqCst));

            // Clean shutdown
            for handle in &handles {
                let _ = handle.stop().await;
            }
            let _ = runtime.shutdown_all().await;
        });
    });
}

// =============================================================================
// Supervision Benchmarks
// =============================================================================

/// Benchmarks spawning child actors under supervision.
#[divan::bench(args = [1, 5, 10, 20])]
fn supervision_spawn_children(bencher: Bencher<'_, '_>, child_count: usize) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    bencher.bench_local(|| {
        rt.block_on(async {
            let mut runtime = ActonApp::launch_async().await;

            // Create parent actor
            let parent = runtime.new_actor_with_name::<CounterActor>("parent".to_string());
            let parent_handle = parent.start().await;

            // Spawn children under supervision
            let mut child_handles = Vec::with_capacity(child_count);
            for i in 0..child_count {
                let child_config = ActorConfig::new(
                    Ern::with_root(format!("child_{i}")).unwrap(),
                    Some(parent_handle.clone()),
                    parent_handle.get_broker(),
                )
                .unwrap();

                let child = runtime.new_actor_with_config::<CounterActor>(child_config);

                let child_handle = parent_handle.supervise(child).await.unwrap();
                child_handles.push(child_handle);
            }

            black_box(&child_handles);

            // Verify children are registered
            assert_eq!(parent_handle.children().len(), child_count);

            // Clean shutdown - stop parent (should cascade to children)
            let _ = parent_handle.stop().await;
            let _ = runtime.shutdown_all().await;
        });
    });
}
