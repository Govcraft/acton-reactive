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
//! legitimately disagree: its own registry, and the `children` map on the handle
//! a `supervise()` call was made through. Each test here covers a case that only
//! one of those two views can see, so a "simplification" to either one alone
//! fails a test rather than silently orphaning a child.

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
/// This is the case the registry exists for. `ActorHandle::clone` deep-copies
/// the `children` map, so a child adopted through such a clone is invisible to
/// the parent's own task — before the registry, it simply outlived its parent.
///
/// The parent is stopped directly rather than through `shutdown_all()`, because
/// `shutdown_all()` stops roots itself and would pass even if the cascade were
/// broken.
#[acton_test]
async fn a_child_supervised_through_a_handle_clone_is_stopped_with_its_parent(
) -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>().start().await;
    // A clone obtained after the parent started: its `children` map is a
    // separate copy that the parent's task can never see.
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

/// A handle clone carries an independent `children` map.
///
/// The mechanism the whole cascade story rests on. `ActorHandle` holds a
/// `DashMap`, whose `Clone` deep-copies, so what one clone supervises is
/// invisible to every other clone — including the one living inside the actor's
/// own task. Pinned here so that a change to how handles clone shows up as a
/// failure with an explanation rather than as an orphaned child.
#[acton_test]
async fn supervising_through_one_clone_is_invisible_to_another() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor::<Parent>().start().await;
    let other_clone = parent.clone();

    let stopped = Arc::new(AtomicBool::new(false));
    let child = spawn_child(&mut runtime, &stopped);
    other_clone.supervise(child).await?;

    assert_eq!(other_clone.children().len(), 1);
    assert_eq!(
        parent.children().len(),
        0,
        "a clone's children map is its own"
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
/// The case where the parent is stopped before it drains that message is *not*
/// covered here, because it is still not fixed. `ManagedActor::supervise_with`
/// was meant to be the answer — it records synchronously — but it is
/// `async fn(&mut self)` and a handler's asynchronous half is a `'static`
/// future that cannot borrow the actor, so no user handler can call it.
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
