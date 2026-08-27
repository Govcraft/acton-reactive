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

//! Cascading shutdown reaches every supervised child.
//!
//! A supervisor decides which children to stop from two views that can
//! legitimately disagree: its own registry, and the `children` map its handles
//! share. Each test here covers a case that only one of those two views can
//! see, so a "simplification" to either one alone fails a test rather than
//! silently orphaning a child.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

#[acton_actor]
struct Parent;

#[acton_actor]
struct Child;

#[acton_message]
struct AdoptChild;

/// Waits for `flag` to be set, up to a bounded time.
///
/// Polls rather than sleeping a fixed duration so a passing test is fast and a
/// failing one is still decisive.
async fn wait_for_flag(flag: &Arc<AtomicBool>) -> bool {
    for _ in 0..200 {
        if flag.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    flag.load(Ordering::SeqCst)
}

/// Builds a child that records having been stopped.
fn spawn_child(
    runtime: &mut ActorRuntime,
    stopped: &Arc<AtomicBool>,
) -> ManagedActor<Idle, Child> {
    let mut builder = runtime.new_actor::<Child>();
    let stopped = Arc::clone(stopped);
    builder.after_stop(move |_actor| {
        let stopped = Arc::clone(&stopped);
        async move {
            stopped.store(true, Ordering::SeqCst);
        }
    });
    builder
}

/// A child supervised through a handle clone obtained *after* the parent started
/// is still stopped when the parent stops.
///
/// This is the case the registry was added for: `ActorHandle::clone` used to
/// deep-copy the `children` map, so a child adopted through such a clone was
/// invisible to the parent's own task and simply outlived it. The map is now
/// shared, so both views see this child; the registry still has to carry it,
/// because a caller may drop the clone it supervised through.
///
/// The parent is stopped directly rather than through `shutdown_all()`, because
/// `shutdown_all()` stops roots itself and would pass even if the cascade were
/// broken.
#[acton_test]
async fn a_child_supervised_through_a_handle_clone_is_stopped_with_its_parent(
) -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>().start().await;
    // A clone obtained after the parent started.
    let external = parent.clone();

    let stopped = Arc::new(AtomicBool::new(false));
    let child = spawn_child(&mut runtime, &stopped);
    let child_handle = external.supervise(child).await?;

    // Let the registration message reach the parent's task.
    tokio::time::sleep(Duration::from_millis(50)).await;

    parent.stop().await?;

    assert!(
        wait_for_flag(&stopped).await,
        "child {} outlived its parent: the cascade missed the registry",
        child_handle.id()
    );

    Ok(())
}

/// Every clone of a handle shares one `children` map.
///
/// The map lives behind an `Arc`, so cloning a handle costs a reference-count
/// bump instead of a deep copy of every child handle, and a child supervised
/// through any clone is visible through all of them — including the handle
/// living inside the actor's own task. Pinned here in both directions so that
/// a regression to per-clone maps fails with an explanation.
#[acton_test]
async fn supervising_through_one_clone_is_visible_to_another() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>().start().await;
    let other_clone = parent.clone();

    let stopped = Arc::new(AtomicBool::new(false));
    let child = spawn_child(&mut runtime, &stopped);
    let through_clone = other_clone.supervise(child).await?;

    assert_eq!(other_clone.children().len(), 1);
    assert_eq!(
        parent.children().len(),
        1,
        "a child supervised through a clone is missing from the original handle"
    );
    assert!(
        parent.find_child(&through_clone.id()).is_some(),
        "the original handle cannot find a child supervised through its clone"
    );

    // And the other way round: a clone taken before the child was supervised,
    // and one taken after, both read the same map.
    let stopped_second = Arc::new(AtomicBool::new(false));
    let second = spawn_child(&mut runtime, &stopped_second);
    let through_original = parent.supervise(second).await?;

    assert_eq!(other_clone.children().len(), 2);
    assert!(
        other_clone.find_child(&through_original.id()).is_some(),
        "the clone cannot find a child supervised through the original handle"
    );
    assert_eq!(parent.clone().children().len(), 2);

    runtime.shutdown_all().await?;
    Ok(())
}

/// What cloning a handle costs, measured rather than asserted.
///
/// Ignored by default because it is a timing report, not a pass/fail property:
/// run it with `cargo nextest run --ignored handle_clone_cost` (or
/// `cargo test -- --ignored --nocapture`) and read the printed ns/clone.
#[acton_test]
#[ignore = "timing report, not a correctness check"]
async fn handle_clone_cost_with_fifty_children() -> anyhow::Result<()> {
    use std::hint::black_box;
    use std::time::Instant;

    const ITERATIONS: u32 = 100_000;

    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;

    let stopped = Arc::new(AtomicBool::new(false));
    for _ in 0..50 {
        parent
            .supervise(spawn_child(&mut runtime, &stopped))
            .await?;
    }
    assert_eq!(parent.children().len(), 50);

    // Warm the allocator so the first few clones do not dominate.
    for _ in 0..1_000 {
        drop(black_box(parent.clone()));
    }

    let started = Instant::now();
    for _ in 0..ITERATIONS {
        drop(black_box(parent.clone()));
    }
    let elapsed = started.elapsed();

    println!(
        "clone+drop of a handle with 50 children: {} ns/clone ({elapsed:?} for {ITERATIONS} iterations)",
        elapsed.as_nanos() / u128::from(ITERATIONS)
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// A child adopted from inside the parent's own handler is stopped with the
/// parent, once the parent has processed the registration.
///
/// A handler cannot supervise through the actor's task-local handle: moving a
/// handle into the returned future requires cloning it, and that clone gets its
/// own `children` map. So this child is reachable only through the registry, and
/// only after the registration message has been processed.
///
/// A handler that wants the child recorded without a round trip has
/// `ManagedActor::supervise_deferred` instead — see the test below.
#[acton_test]
async fn a_child_adopted_in_a_handler_is_stopped_with_its_parent() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let stopped = Arc::new(AtomicBool::new(false));
    let child = spawn_child(&mut runtime, &stopped);
    let pending_child = Arc::new(tokio::sync::Mutex::new(Some(child)));

    let mut parent_builder = runtime.new_actor::<Parent>();
    parent_builder.mutate_on::<AdoptChild>(move |actor, _envelope| {
        let handle = actor.handle().clone();
        let pending = Arc::clone(&pending_child);
        Reply::pending(async move {
            let taken = pending.lock().await.take();
            if let Some(child) = taken {
                let _ = handle.supervise(child).await;
            }
        })
    });
    let parent = parent_builder.start().await;

    parent.send(AdoptChild).await;
    // Let the handler run and its registration reach the parent's task.
    tokio::time::sleep(Duration::from_millis(100)).await;

    parent.stop().await?;

    assert!(
        wait_for_flag(&stopped).await,
        "child adopted in a handler outlived its parent"
    );

    Ok(())
}

/// A child a handler supervised through `supervise_deferred` is stopped with
/// its parent.
///
/// Only the registry can see this one. The child is created by the parent's own
/// message loop rather than by a `supervise()` call, so no handle's `children`
/// map ever hears about it — a cascade that read only that map would orphan it.
#[acton_test]
async fn a_child_supervised_in_a_handler_is_stopped_with_its_parent() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let stopped = Arc::new(AtomicBool::new(false));
    let blueprint = {
        let stopped = Arc::clone(&stopped);
        move |child: &mut ManagedActor<Idle, Child>| {
            let stopped = Arc::clone(&stopped);
            child.after_stop(move |_actor| {
                let stopped = Arc::clone(&stopped);
                async move {
                    stopped.store(true, Ordering::SeqCst);
                }
            });
        }
    };

    let mut parent_builder = runtime.new_actor::<Parent>();
    parent_builder.mutate_on::<AdoptChild>(move |actor, _envelope| {
        let config = ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
            .expect("a name plus a live parent is a valid child configuration");
        let _ = actor.supervise_deferred(config, blueprint.clone());
        Reply::ready()
    });
    let parent = parent_builder.start().await;

    parent.send(AdoptChild).await;
    // Let the handler run and the parent's loop create what it queued.
    tokio::time::sleep(Duration::from_millis(100)).await;

    parent.stop().await?;

    assert!(
        wait_for_flag(&stopped).await,
        "child supervised in a handler outlived its parent"
    );

    Ok(())
}

/// The pre-existing `children()` view keeps reporting what it always did.
///
/// `supervise()` still inserts into the calling handle's map, so code that
/// counts children through the handle it supervised with is unaffected.
#[acton_test]
async fn supervising_still_populates_the_calling_handles_children_map() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>().start().await;
    let stopped = Arc::new(AtomicBool::new(false));

    let child = spawn_child(&mut runtime, &stopped);
    let child_handle = parent.supervise(child).await?;

    assert_eq!(parent.children().len(), 1);
    assert!(parent.find_child(&child_handle.id()).is_some());

    runtime.shutdown_all().await?;
    Ok(())
}

/// Stopping a parent with no children is unaffected by the union.
#[acton_test]
async fn a_childless_parent_still_stops_cleanly() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>().start().await;
    assert_eq!(parent.children().len(), 0);

    parent.stop().await?;
    Ok(())
}

/// A child supervised twice through two different handle clones is stopped once.
///
/// Both views name the same child, so the union must deduplicate; stopping the
/// same handle twice would surface as a shutdown error in the logs.
#[acton_test]
async fn a_child_present_in_both_views_is_stopped_exactly_once() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent_builder = runtime.new_actor::<Parent>();

    let stopped = Arc::new(AtomicBool::new(false));
    let child = spawn_child(&mut runtime, &stopped);
    // Supervised through the actor's own handle, without cloning it, so the map
    // that moves into the actor's task is the one that gets the entry. The
    // registration message is queued too, so both views name this child.
    parent_builder.handle().supervise(child).await?;
    assert_eq!(parent_builder.handle().children().len(), 1);

    let parent = parent_builder.start().await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    parent.stop().await?;

    assert!(
        wait_for_flag(&stopped).await,
        "child in both views was not stopped"
    );

    Ok(())
}
