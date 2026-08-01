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

//! Keeping the IPC registry honest as actors restart, die, and are handed back.
//!
//! Four pieces of machinery decide whether a name in the IPC namespace still
//! points at something that can answer:
//!
//! | mechanism | question it settles |
//! |---|---|
//! | `ActorRuntime::ipc_rebind` | a restarted child keeps answering |
//! | `ActorRuntime::ipc_forget` | a departed child stops answering |
//! | `SupervisionEngine::forget_ipc_names` | which terminal children lose names |
//! | `SupervisionEngine::forget_children_ipc_names` | a supervisor on its way down |
//!
//! # Liveness is the only honest proof, and equality is a trap
//!
//! [`ActorHandle`] implements `PartialEq` over its [`Ern`] alone, and a restart
//! **keeps** the `Ern` while replacing the mailbox. A stale handle and the
//! incarnation that superseded it are therefore *equal*. Any freshness check
//! written as `assert_eq!(a, b)` or `assert_ne!(a, b)` passes identically whether
//! or not the registry was ever repointed — it proves nothing while reading as
//! though it proves everything.
//!
//! So every "the name still works" claim here routes a [`Probe`] through the
//! handle the registry hands back and waits for a counter the *receiving message
//! loop* increments. A send into a dead mailbox is accepted and never handled,
//! which is exactly the silent failure this machinery exists to prevent, and the
//! only thing that distinguishes the two cases is whether anybody ran.
//!
//! # Every test that asserts a change also asserts what stayed put
//!
//! `ipc_rebind` and `ipc_forget` both filter by `entry.value().id() == *child`
//! before touching anything. Without that filter, restarting or burying one actor
//! would repoint or wipe **every** name in the namespace. A test that only checks
//! the actor it acted on cannot see that, so each one here also pins a bystander.
//!
//! The bystander assertion is deliberately **positive** — the name is still there,
//! still resolving to the same actor, and that actor still answers. An absence
//! check such as `assert!(lookup("beta").is_none())` would pass *harder* if the
//! filter were removed and everything got wiped, so it cannot catch the one
//! mutation it appears to be guarding.
//!
//! # Names are chosen, never derived
//!
//! These tests call [`ActorRuntime::ipc_expose`] with names of their own rather
//! than using `expose_for_ipc()`, so that nothing here depends on the shape of a
//! derived child identifier. That derivation is changing, and a test pinning
//! today's spelling would fail for a reason it does not describe.

#![cfg(feature = "ipc")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a coffee break.
const PATIENCE: Duration = Duration::from_secs(5);

#[acton_actor]
struct Parent;

#[acton_actor]
struct Worker;

/// Asks whichever incarnation receives it to record that it was alive to do so.
///
/// The counter rides along inside the message rather than living in the actor's
/// model, because a restart builds a fresh model and would reset it. What is
/// being measured is that *somebody ran*, not what they were holding.
#[acton_message]
struct Probe {
    hits: Arc<AtomicUsize>,
}

/// Restart settings that make a test quick and its arithmetic obvious.
const fn brisk_limiter(max_restarts: u32) -> RestartLimiterConfig {
    RestartLimiterConfig {
        enabled: true,
        max_restarts,
        window_secs: 60,
        initial_backoff_ms: 10,
        max_backoff_ms: 50,
        backoff_multiplier: 1.0,
    }
}

/// A blueprint whose children answer probes and count how many were built.
///
/// The build count is the proof that a restart produced a *new* incarnation
/// rather than the old one being revived — nothing else distinguishes them,
/// because a restart deliberately keeps the child's identifier.
fn worker_blueprint(
    builds: &Arc<AtomicUsize>,
) -> impl Fn(&mut ManagedActor<Idle, Worker>) + Clone + Send + Sync + 'static {
    let builds = Arc::clone(builds);
    move |actor: &mut ManagedActor<Idle, Worker>| {
        builds.fetch_add(1, Ordering::SeqCst);
        actor.mutate_on::<Probe>(|_actor, context| {
            context.message().hits.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });
    }
}

/// A child configuration with an explicit policy and a brisk limiter.
fn child_config(name: &str, parent: &ActorHandle, policy: RestartPolicy) -> ActorConfig {
    ActorConfig::for_supervised_child(name, parent.clone(), None)
        .expect("a name plus a live parent is a valid child configuration")
        .with_restart_policy(policy)
        .with_restart_limiter(brisk_limiter(5))
}

/// Whether a live message loop is on the other end of this handle.
///
/// Returns `false` rather than hanging when nobody is: a send into an orphaned
/// mailbox succeeds and is simply never handled, so the absence of an answer is
/// the observation, and it has to be bounded to be one.
async fn answers(handle: &ActorHandle) -> bool {
    let hits = Arc::new(AtomicUsize::new(0));
    let delivery = handle.send(Probe {
        hits: Arc::clone(&hits),
    });
    if tokio::time::timeout(PATIENCE, delivery).await.is_err() {
        return false;
    }

    let deadline = tokio::time::Instant::now() + PATIENCE;
    while tokio::time::Instant::now() < deadline {
        if hits.load(Ordering::SeqCst) > 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Waits for a name to leave the registry, so a removal is awaited rather than raced.
///
/// The removal happens on the supervisor's task in response to a termination it
/// has not necessarily processed yet when the stop call returns.
async fn awaits_removal(runtime: &ActorRuntime, name: &str) -> bool {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while tokio::time::Instant::now() < deadline {
        if runtime.ipc_lookup(name).is_none() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Asserts a bystander was left completely alone: same name, same actor, still serving.
///
/// Positive on purpose. The mutation this exists to catch — dropping the identity
/// filter — *removes* entries, so an assertion that the bystander is absent would
/// be satisfied by the very bug it is meant to detect.
async fn untouched(runtime: &ActorRuntime, name: &str, expected: &ActorHandle) {
    let found = runtime
        .ipc_lookup(name)
        .unwrap_or_else(|| panic!("'{name}' belongs to an actor nobody acted on and must remain"));
    assert_eq!(
        found.id(),
        expected.id(),
        "'{name}' must still resolve to the actor that claimed it"
    );
    assert!(
        answers(&found).await,
        "'{name}' must still reach a live actor, not merely still be present"
    );
}

// ============================================================================
// ipc_rebind — a restarted child keeps answering
// ============================================================================

/// A restart repoints the name at the incarnation that can actually answer.
///
/// Before `ipc_rebind`, `ipc_expose` stored a handle *by value* and nothing
/// updated it, so an actor became unreachable over IPC from its first restart
/// onward and reported nothing.
///
/// Note that the registered handle compares **equal** before and after the
/// repoint, so the probe is not a nicety here; it is the entire assertion.
#[tokio::test]
async fn a_restarted_child_still_answers_to_its_ipc_name() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut child = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("alpha", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("the first child of a name is accepted");

    let first = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")
        .expect("the first incarnation is running");
    runtime
        .ipc_expose("alpha", first.clone())
        .expect("IPC name should be unclaimed at startup");
    assert!(
        answers(&first).await,
        "precondition: it answers to begin with"
    );

    first.stop().await.expect("stop the first incarnation");
    tokio::time::timeout(
        PATIENCE,
        child.wait_generation(RestartGeneration::FIRST.next()),
    )
    .await
    .expect("the framework must bring a Permanent child back")
    .expect("the replacement is running");
    assert_eq!(
        builds.load(Ordering::SeqCst),
        2,
        "the blueprint ran again, so this is a new incarnation and not the old one"
    );

    let exposed = runtime
        .ipc_lookup("alpha")
        .expect("a restart must not cost the child its name");
    assert!(
        answers(&exposed).await,
        "the name must reach the live incarnation; an equal-but-stale handle is the defect"
    );

    runtime.shutdown_all().await.expect("shutdown");
}

/// Restarting one actor leaves every other actor's name exactly where it was.
///
/// This is the identity filter. Without `entry.value().id() == *child`,
/// `ipc_rebind` repoints the whole namespace at whichever child happened to
/// restart, and every unrelated name starts delivering to the wrong actor.
#[tokio::test]
async fn a_restart_does_not_repoint_an_unrelated_actors_name() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut restarting = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("alpha", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("alpha is accepted");
    let mut bystander = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("beta", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("beta is accepted");

    let alpha = tokio::time::timeout(PATIENCE, restarting.wait_running())
        .await
        .expect("alpha starts")
        .expect("alpha is running");
    let beta = tokio::time::timeout(PATIENCE, bystander.wait_running())
        .await
        .expect("beta starts")
        .expect("beta is running");
    runtime
        .ipc_expose("alpha", alpha.clone())
        .expect("alpha name");
    runtime.ipc_expose("beta", beta.clone()).expect("beta name");

    alpha.stop().await.expect("stop alpha");
    tokio::time::timeout(
        PATIENCE,
        restarting.wait_generation(RestartGeneration::FIRST.next()),
    )
    .await
    .expect("alpha must come back")
    .expect("alpha's replacement is running");

    // What changed.
    let repointed = runtime.ipc_lookup("alpha").expect("alpha keeps its name");
    assert!(
        answers(&repointed).await,
        "alpha's name reaches the new alpha"
    );

    // What must not have. `beta` never restarted and never terminated; if its
    // name now resolves to alpha, the rebind ignored whose name it was.
    untouched(&runtime, "beta", &beta).await;

    runtime.shutdown_all().await.expect("shutdown");
}

/// One actor exposed under several names gets all of them repointed.
///
/// The filter matches every entry naming the child, so there is no first match to
/// stop at. An implementation that repointed only one would leave the rest
/// pointing at a mailbox with no reader.
#[tokio::test]
async fn a_restart_repoints_every_name_the_child_answers_to() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut child = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("alpha", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("alpha is accepted");
    let first = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("alpha starts")
        .expect("alpha is running");

    // Same actor, three ways to address it.
    for name in ["prices", "prices-primary", "quotes"] {
        runtime
            .ipc_expose(name, first.clone())
            .expect("IPC name should be unclaimed at startup");
    }

    first.stop().await.expect("stop alpha");
    tokio::time::timeout(
        PATIENCE,
        child.wait_generation(RestartGeneration::FIRST.next()),
    )
    .await
    .expect("alpha must come back")
    .expect("alpha's replacement is running");

    for name in ["prices", "prices-primary", "quotes"] {
        let exposed = runtime
            .ipc_lookup(name)
            .unwrap_or_else(|| panic!("'{name}' must survive the restart"));
        assert!(
            answers(&exposed).await,
            "'{name}' must reach the live incarnation too, not just the first name found"
        );
    }

    runtime.shutdown_all().await.expect("shutdown");
}

// ============================================================================
// forget_ipc_names — a child that is not coming back stops answering
// ============================================================================

/// A child whose policy forbids a restart loses its names when it dies.
///
/// Callers otherwise send into a mailbox nobody reads and are told nothing; with
/// the name gone they are told there is no such actor, which is true.
#[tokio::test]
async fn a_child_recorded_down_stops_answering_to_its_ipc_name() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut doomed = parent
        .supervise_with::<Worker>(
            &runtime,
            // Temporary is never restarted, so its termination is terminal.
            child_config("alpha", &parent, RestartPolicy::Temporary),
            worker_blueprint(&builds),
        )
        .await
        .expect("alpha is accepted");
    let mut bystander = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("beta", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("beta is accepted");

    let alpha = tokio::time::timeout(PATIENCE, doomed.wait_running())
        .await
        .expect("alpha starts")
        .expect("alpha is running");
    let beta = tokio::time::timeout(PATIENCE, bystander.wait_running())
        .await
        .expect("beta starts")
        .expect("beta is running");
    runtime
        .ipc_expose("alpha", alpha.clone())
        .expect("alpha name");
    runtime.ipc_expose("beta", beta.clone()).expect("beta name");

    alpha.stop().await.expect("stop alpha");
    let status = tokio::time::timeout(PATIENCE, doomed.wait_for(|s| s.state().is_terminal()))
        .await
        .expect("a Temporary child must reach a terminal state")
        .expect("alpha settled");
    assert_eq!(
        status.state(),
        SupervisionState::Down,
        "precondition: this test is about the Down road, not the escalation one"
    );

    assert!(
        awaits_removal(&runtime, "alpha").await,
        "a child that is not coming back must stop answering to its name"
    );
    untouched(&runtime, "beta", &beta).await;

    runtime.shutdown_all().await.expect("shutdown");
}

/// A child that burns through its restart allowance also loses its names.
///
/// The other terminal road into `forget_ipc_names`, and a separate call site from
/// the one above: giving up is not the same code path as never trying.
#[tokio::test]
async fn an_escalated_child_stops_answering_to_its_ipc_name() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut doomed = parent
        .supervise_with::<Worker>(
            &runtime,
            ActorConfig::for_supervised_child("alpha", parent.clone(), None)
                .expect("valid child configuration")
                .with_restart_policy(RestartPolicy::Permanent)
                .with_restart_limiter(brisk_limiter(1)),
            worker_blueprint(&builds),
        )
        .await
        .expect("alpha is accepted");
    let mut bystander = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("beta", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("beta is accepted");

    let mut alpha = tokio::time::timeout(PATIENCE, doomed.wait_running())
        .await
        .expect("alpha starts")
        .expect("alpha is running");
    let beta = tokio::time::timeout(PATIENCE, bystander.wait_running())
        .await
        .expect("beta starts")
        .expect("beta is running");
    runtime
        .ipc_expose("alpha", alpha.clone())
        .expect("alpha name");
    runtime.ipc_expose("beta", beta.clone()).expect("beta name");

    // One restart is the whole allowance; the second failure has nothing left.
    alpha.stop().await.expect("first stop");
    alpha = tokio::time::timeout(
        PATIENCE,
        doomed.wait_generation(RestartGeneration::FIRST.next()),
    )
    .await
    .expect("the one restart the allowance covers")
    .expect("alpha's replacement is running");
    alpha.stop().await.expect("second stop");

    let status = tokio::time::timeout(PATIENCE, doomed.wait_for(|s| s.state().is_terminal()))
        .await
        .expect("an exhausted child must reach a terminal state")
        .expect("alpha settled");
    assert_eq!(
        status.state(),
        SupervisionState::Escalated,
        "precondition: this test is about the escalation road, not the Down one"
    );

    assert!(
        awaits_removal(&runtime, "alpha").await,
        "a child the supervisor gave up on must stop answering to its name"
    );
    untouched(&runtime, "beta", &beta).await;

    runtime.shutdown_all().await.expect("shutdown");
}

/// A child the engine holds no blueprint for keeps its names when it dies.
///
/// `forget_ipc_names` sweeps only engine-managed children — the ones registered
/// through `supervise_with` and `supervise_deferred`, which are the ones with a
/// spawner. A child adopted through the legacy `supervise()` path keeps its names
/// exactly as it does today.
///
/// # Why both children are in one test
///
/// Asserting only that the legacy child kept its name would pass just as well if
/// the sweep never ran at all — the dominant way a test like this certifies
/// nothing. The engine-managed sibling is the control: it dies the same way in the
/// same runtime, so its name disappearing proves the sweep genuinely ran, which is
/// what makes the legacy child's name surviving attributable to the guard.
#[tokio::test]
async fn a_child_without_a_blueprint_keeps_its_ipc_names() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    // The control: engine-managed, so the sweep is entitled to it.
    let mut managed = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("managed", &parent, RestartPolicy::Temporary),
            worker_blueprint(&builds),
        )
        .await
        .expect("the managed child is accepted");
    let managed_handle = tokio::time::timeout(PATIENCE, managed.wait_running())
        .await
        .expect("the managed child starts")
        .expect("the managed child is running");

    // The subject: adopted through the legacy path, so it has no spawner and
    // `is_restartable` is false for it.
    let legacy_config = child_config("legacy", &parent, RestartPolicy::Temporary);
    let mut legacy_actor = runtime.new_actor_with_config::<Worker>(legacy_config);
    legacy_actor.mutate_on::<Probe>(|_actor, context| {
        context.message().hits.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });
    let legacy_id = legacy_actor.id().clone();
    let legacy_handle = parent
        .supervise(legacy_actor)
        .await
        .expect("the legacy child is adopted");

    runtime
        .ipc_expose("managed", managed_handle.clone())
        .expect("managed name");
    runtime
        .ipc_expose("legacy", legacy_handle.clone())
        .expect("legacy name");
    assert_eq!(
        legacy_handle.id(),
        legacy_id,
        "precondition: the adopted handle is the child that was built"
    );

    // Legacy first, so that by the time the managed child's termination has been
    // processed the legacy one's already has: a supervisor takes its inbox in
    // order, and both terminations are addressed to the same supervisor.
    legacy_handle.stop().await.expect("stop the legacy child");
    managed_handle.stop().await.expect("stop the managed child");
    tokio::time::timeout(PATIENCE, managed.wait_for(|s| s.state().is_terminal()))
        .await
        .expect("the managed child must reach a terminal state")
        .expect("the managed child settled");

    assert!(
        awaits_removal(&runtime, "managed").await,
        "control: an engine-managed child loses its names, so the sweep did run"
    );

    let kept = runtime
        .ipc_lookup("legacy")
        .expect("a child the engine holds no blueprint for must keep its names");
    assert_eq!(
        kept.id(),
        legacy_id,
        "and the name must still resolve to that child rather than being reused"
    );

    runtime.shutdown_all().await.expect("shutdown");
}

// ============================================================================
// unregister_supervised_child — released to keep serving, or on its way out
// ============================================================================

/// Unsupervising a child takes its names with it, because it is being stopped.
#[tokio::test]
async fn unsupervising_a_child_removes_its_ipc_names() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut going = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("alpha", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("alpha is accepted");
    let mut staying = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("beta", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("beta is accepted");

    let alpha = tokio::time::timeout(PATIENCE, going.wait_running())
        .await
        .expect("alpha starts")
        .expect("alpha is running");
    let beta = tokio::time::timeout(PATIENCE, staying.wait_running())
        .await
        .expect("beta starts")
        .expect("beta is running");
    runtime
        .ipc_expose("alpha", alpha.clone())
        .expect("alpha name");
    runtime.ipc_expose("beta", beta.clone()).expect("beta name");

    parent
        .unsupervise(&alpha.id())
        .await
        .expect("unsupervise stops the child and retires its slot");

    assert!(
        awaits_removal(&runtime, "alpha").await,
        "a child on its way out must stop answering to its names"
    );
    untouched(&runtime, "beta", &beta).await;

    runtime.shutdown_all().await.expect("shutdown");
}

/// Releasing a child leaves it serving, names and all.
///
/// The mirror of the test above, and the reason `unregister_supervised_child`
/// consults `message.stopping` at all: `release` retires the slot but leaves the
/// actor running, so removing its names would strand a healthy actor that is
/// still perfectly able to answer them.
#[tokio::test]
async fn a_released_child_keeps_serving_under_its_ipc_names() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut released = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("alpha", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("alpha is accepted");
    let alpha = tokio::time::timeout(PATIENCE, released.wait_running())
        .await
        .expect("alpha starts")
        .expect("alpha is running");
    runtime
        .ipc_expose("alpha", alpha.clone())
        .expect("alpha name");

    let returned = parent
        .release(&alpha.id())
        .await
        .expect("release hands the child back")
        .expect("the supervisor held a running child");
    assert_eq!(
        returned.id(),
        alpha.id(),
        "precondition: the release returned the child under test"
    );

    // Positive, and liveness rather than presence: the whole point of keeping the
    // name is that something is still there to answer it.
    untouched(&runtime, "alpha", &alpha).await;

    returned
        .stop()
        .await
        .expect("the caller stops it in its own time");
    runtime.shutdown_all().await.expect("shutdown");
}

// ============================================================================
// forget_children_ipc_names — the supervisor itself goes down
// ============================================================================

/// A supervisor stopping inside a still-running system takes its children's
/// names down with it.
///
/// The terminal-state sweep cannot reach these. A cascading shutdown marks the
/// registry `shutting_down`, which makes every termination an expected stop,
/// which means no slot ever reaches `Down` and `forget_ipc_names` is never
/// called — so without this sweep the children would be stopped with their names
/// still pointing at dead mailboxes.
#[tokio::test]
async fn a_stopping_supervisor_drops_its_childrens_ipc_names() {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor::<Parent>().start().await;
    let builds = Arc::new(AtomicUsize::new(0));

    let mut child = parent
        .supervise_with::<Worker>(
            &runtime,
            child_config("alpha", &parent, RestartPolicy::Permanent),
            worker_blueprint(&builds),
        )
        .await
        .expect("alpha is accepted");
    let alpha = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("alpha starts")
        .expect("alpha is running");
    runtime
        .ipc_expose("alpha", alpha.clone())
        .expect("alpha name");

    // An actor with no relationship to the stopping supervisor at all.
    let mut outsider_actor = runtime.new_actor::<Worker>();
    outsider_actor.mutate_on::<Probe>(|_actor, context| {
        context.message().hits.fetch_add(1, Ordering::SeqCst);
        Reply::ready()
    });
    let outsider = outsider_actor.start().await;
    runtime
        .ipc_expose("outsider", outsider.clone())
        .expect("outsider name");

    // The supervisor stops; the runtime keeps going.
    parent.stop().await.expect("stop the supervisor");

    assert!(
        awaits_removal(&runtime, "alpha").await,
        "a child stopped by its supervisor's shutdown must not keep its name"
    );
    untouched(&runtime, "outsider", &outsider).await;

    runtime.shutdown_all().await.expect("shutdown");
}
