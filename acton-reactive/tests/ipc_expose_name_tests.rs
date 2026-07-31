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

//! Tests for issue #13: `expose_for_ipc()` registered a name that was both
//! unusable and colliding.
//!
//! It derived the name from the actor's `Ern` **root**, which carries a generated
//! `UUIDv7` suffix — so the name changed on every process start and no client could
//! ever address it — while discarding the parts, which are the only thing telling a
//! supervised child apart from its parent and its siblings.

#![cfg(feature = "ipc")]

use acton_reactive::prelude::*;
use acton_reactive::ipc::IpcNameInUse;

#[derive(Debug, Default)]
struct Service;

/// A root actor is reachable under the plain name it was given.
///
/// This is what the documentation promised all along and what the code did not do.
#[tokio::test]
async fn a_root_actor_is_exposed_under_its_plain_name() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut service = runtime.new_actor_with_name::<Service>("prices".to_string());
    service.expose_for_ipc();
    let handle = service.start().await;

    let exposed = runtime.ipc_lookup("prices").expect("reachable as 'prices'");
    assert_eq!(exposed.id(), handle.id());
}

/// The generated suffix never leaks into the name.
///
/// It is regenerated every process start, so a name containing it could not be
/// written into a client or a config file — which is what made the old behaviour
/// unusable rather than merely wrong.
#[tokio::test]
async fn the_generated_suffix_is_not_part_of_the_name() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut service = runtime.new_actor_with_name::<Service>("prices".to_string());
    service.expose_for_ipc();
    let handle = service.start().await;

    let root = handle.id().root().as_str().to_owned();
    assert!(
        root.starts_with("prices_"),
        "precondition: the root carries a generated suffix, got {root}"
    );
    assert!(
        runtime.ipc_lookup(&root).is_none(),
        "the suffixed root must not be a registered name"
    );
}

/// Two actors that choose the same name are a genuine conflict, and the conflict
/// is reported instead of one silently replacing the other.
#[tokio::test]
async fn a_duplicate_name_is_refused_and_the_first_actor_keeps_serving() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let first = runtime.new_actor_with_name::<Service>("prices".to_string());
    let first_handle = first.start().await;
    runtime
        .ipc_expose("prices", first_handle.clone())
        .expect("first claim succeeds");

    let second = runtime.new_actor_with_name::<Service>("prices".to_string());
    let second_handle = second.start().await;

    let conflict: IpcNameInUse = runtime
        .ipc_expose("prices", second_handle.clone())
        .expect_err("the name is already claimed");

    assert_eq!(conflict.name(), "prices");
    assert_eq!(
        conflict.held_by(),
        &first_handle.id(),
        "the error names the actor that already holds it"
    );

    // The point of refusing: the actor already serving is still the one serving.
    let exposed = runtime.ipc_lookup("prices").expect("still registered");
    assert_eq!(
        exposed.id(),
        first_handle.id(),
        "the first registration must not be displaced"
    );
    assert_ne!(exposed.id(), second_handle.id());
}

/// The error explains itself well enough to act on.
#[tokio::test]
async fn the_conflict_error_names_the_holder() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let first = runtime.new_actor_with_name::<Service>("prices".to_string());
    let first_handle = first.start().await;
    runtime.ipc_expose("prices", first_handle.clone()).expect("first");

    let second = runtime.new_actor_with_name::<Service>("prices".to_string());
    let second_handle = second.start().await;
    let conflict = runtime
        .ipc_expose("prices", second_handle)
        .expect_err("conflict");

    let rendered = conflict.to_string();
    assert!(rendered.contains("prices"), "should name the name: {rendered}");
    assert!(
        rendered.contains(&first_handle.id().to_string()),
        "should name the holder: {rendered}"
    );
}

/// Hiding a name releases it, so the conflict is recoverable rather than permanent.
#[tokio::test]
async fn hiding_a_name_frees_it_for_another_actor() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let first = runtime.new_actor_with_name::<Service>("prices".to_string());
    let first_handle = first.start().await;
    runtime.ipc_expose("prices", first_handle).expect("first");

    runtime.ipc_hide("prices").expect("the name was registered");

    let second = runtime.new_actor_with_name::<Service>("prices".to_string());
    let second_handle = second.start().await;

    runtime
        .ipc_expose("prices", second_handle.clone())
        .expect("the name is free again");

    assert_eq!(
        runtime.ipc_lookup("prices").expect("registered").id(),
        second_handle.id()
    );
}

// ============================================================================
// Supervised children — the shape that actually collided
// ============================================================================
//
// These use the supervised path (`ActorConfig::for_supervised_child`, which builds
// the child as `parent.add_part(name)`) because that is where the collision lived:
// such a child shares its parent's `Ern` root and is distinguished only by its
// parts, which the old derivation discarded.
//
// They deliberately do not use `create_child`. That path currently produces a
// parts-less identifier, and #12/#15 will change its shape to `parent/child`. A
// test pinning today's form would fail when #15 lands and would look like #15 broke
// this, when really the test would have encoded a bug as a requirement.

/// A supervised child is exposed under its parent's name, then its own.
#[tokio::test]
async fn a_supervised_child_is_exposed_beneath_its_parent() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor_with_name::<Service>("prices".to_string());
    let parent_handle = parent.start().await;

    let config = ActorConfig::for_supervised_child("alpha", parent_handle.clone(), None)
        .expect("child config");
    let child = parent_handle
        .supervise_with::<Service>(&runtime, config, |actor| {
            actor.expose_for_ipc();
        })
        .await
        .expect("supervise child");

    let child_handle = child.current().expect("child running");
    let exposed = runtime
        .ipc_lookup("prices/alpha")
        .expect("reachable as 'prices/alpha'");
    assert_eq!(exposed.id(), child_handle.id());
}

/// Two children of one parent get distinct names, and each resolves to itself.
///
/// Before the fix both registered under the parent's suffixed root, so the second
/// silently replaced the first and messages for one were delivered to the other.
#[tokio::test]
async fn two_supervised_siblings_do_not_collide() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let parent = runtime.new_actor_with_name::<Service>("prices".to_string());
    let parent_handle = parent.start().await;

    for name in ["alpha", "beta"] {
        let config = ActorConfig::for_supervised_child(name, parent_handle.clone(), None)
            .expect("child config");
        parent_handle
            .supervise_with::<Service>(&runtime, config, |actor| {
                actor.expose_for_ipc();
            })
            .await
            .expect("supervise child");
    }

    let alpha = runtime.ipc_lookup("prices/alpha").expect("alpha registered");
    let beta = runtime.ipc_lookup("prices/beta").expect("beta registered");

    assert_ne!(
        alpha.id(),
        beta.id(),
        "siblings must resolve to different actors"
    );
}

/// A child does not displace its parent.
///
/// Before the fix the child registered under the parent's own name, so exposing any
/// child made the parent unreachable.
#[tokio::test]
async fn a_supervised_child_does_not_displace_its_parent() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let mut parent = runtime.new_actor_with_name::<Service>("prices".to_string());
    parent.expose_for_ipc();
    let parent_handle = parent.start().await;

    let config = ActorConfig::for_supervised_child("alpha", parent_handle.clone(), None)
        .expect("child config");
    parent_handle
        .supervise_with::<Service>(&runtime, config, |actor| {
            actor.expose_for_ipc();
        })
        .await
        .expect("supervise child");

    let exposed_parent = runtime
        .ipc_lookup("prices")
        .expect("the parent is still reachable");
    assert_eq!(exposed_parent.id(), parent_handle.id());
    assert!(runtime.ipc_lookup("prices/alpha").is_some());
}

/// Distinct names still coexist; refusing duplicates must not refuse everything.
#[tokio::test]
async fn distinct_names_are_both_registered() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;

    let a = runtime.new_actor_with_name::<Service>("prices".to_string());
    let a_handle = a.start().await;
    let b = runtime.new_actor_with_name::<Service>("orders".to_string());
    let b_handle = b.start().await;

    runtime.ipc_expose("prices", a_handle.clone()).expect("prices");
    runtime.ipc_expose("orders", b_handle.clone()).expect("orders");

    assert_eq!(runtime.ipc_lookup("prices").expect("a").id(), a_handle.id());
    assert_eq!(runtime.ipc_lookup("orders").expect("b").id(), b_handle.id());
}
