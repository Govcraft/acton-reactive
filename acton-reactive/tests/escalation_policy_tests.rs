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

//! What a supervisor does once restarting a child has stopped working.
//!
//! `Escalation` shipped in 8.2.0 as public, documented, prelude-exported, and
//! entirely unread: there was no way to set it and nothing consulted it. These
//! tests exist to keep it reachable, so the two policies are told apart by
//! something other than their doc comments.
//!
//! The measurement in every test is what happens to the **supervisor**, because
//! that is the whole difference between the two. What happens to the child is
//! identical either way: it stops being restarted and publishes `Escalated`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a coffee break.
const PATIENCE: Duration = Duration::from_secs(5);

#[acton_actor]
struct Grandparent;

#[acton_actor]
struct Parent;

#[acton_actor]
struct Worker;

/// Asks the supervisor to hire the child that cannot be kept alive.
#[acton_message]
struct HireWorker;

/// Routed to the supervisor to prove it is still taking messages.
#[acton_message]
struct Ping;

/// A restart allowance small enough to exhaust immediately.
///
/// One restart, then the supervisor gives up. The backoff is tiny so the test
/// measures the escalation rather than the waiting.
const fn one_restart_only() -> RestartLimiterConfig {
    RestartLimiterConfig {
        enabled: true,
        max_restarts: 1,
        window_secs: 60,
        initial_backoff_ms: 5,
        max_backoff_ms: 10,
        backoff_multiplier: 1.0,
    }
}

/// Everything a test needs to watch one escalation happen.
struct Escalating {
    runtime: ActorRuntime,
    /// The supervisor whose escalation policy is under test.
    parent: ActorHandle,
    /// The caller's view of the child that will exhaust its allowance.
    child: SupervisedChild,
    /// How many `SupervisionEscalated` messages the grandparent received.
    escalations: Arc<AtomicUsize>,
    /// How many pings the supervisor has answered.
    ///
    /// A supervisor that stopped answers none, which is what distinguishes
    /// `StopSupervisor` from `NotifyParent` after the dust settles.
    pings: Arc<AtomicUsize>,
}

/// Builds a grandparent -> supervisor -> child chain, with the child running.
///
/// The child is `Permanent`, so a plain `stop()` is a termination that warrants
/// a restart — which makes stopping it from the test body a usable stand-in for
/// a crash, and keeps the failures under the test's control rather than racing
/// them. Driving them from the child's own `after_start` does not work: it would
/// await its own task's tracker and deadlock instead of stopping.
async fn escalating_chain(escalation: Escalation) -> anyhow::Result<Escalating> {
    let mut runtime = ActonApp::launch_async().await;

    let escalations = Arc::new(AtomicUsize::new(0));
    let pings = Arc::new(AtomicUsize::new(0));

    // The grandparent exists only to be told.
    let mut grandparent = runtime.new_actor::<Grandparent>();
    {
        let seen = Arc::clone(&escalations);
        grandparent.mutate_on::<SupervisionEscalated>(move |_actor, _ctx| {
            seen.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });
    }
    let grandparent = grandparent.start().await;

    // The supervisor under test, as a child of the grandparent so that it has
    // somebody to notify.
    let parent_config = ActorConfig::for_supervised_child("supervisor", grandparent.clone(), None)?
        .with_escalation(escalation);
    let mut parent = runtime.new_actor_with_config::<Parent>(parent_config);

    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();
    parent.mutate_on::<HireWorker>(move |actor, _ctx| {
        let config = ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
            .expect("a name plus a live parent is a valid child configuration")
            .with_restart_policy(RestartPolicy::Permanent)
            .with_restart_limiter(one_restart_only());

        let child = actor.supervise_deferred(config, |worker: &mut ManagedActor<Idle, Worker>| {
            worker.mutate_on::<Ping>(|_worker, _ctx| Reply::ready());
        });
        let _ = registered.send(child);
        Reply::ready()
    });
    {
        let answered = Arc::clone(&pings);
        parent.mutate_on::<Ping>(move |_actor, _ctx| {
            answered.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });
    }
    let parent = parent.start().await;

    parent.send(HireWorker).await;
    let child = tokio::time::timeout(PATIENCE, registrations.recv())
        .await
        .expect("the supervisor must answer a hire")
        .expect("the channel is open")
        .expect("the first child of a name is accepted");

    Ok(Escalating {
        runtime,
        parent,
        child,
        escalations,
        pings,
    })
}

/// Fails the child until its supervisor gives up, and reports the state reached.
///
/// One restart is the whole allowance, so the second failure has nothing left.
async fn exhaust_the_allowance(child: &mut SupervisedChild) -> anyhow::Result<SupervisionState> {
    let handle = tokio::time::timeout(PATIENCE, child.wait_running())
        .await
        .expect("the first start must land")?;
    handle.stop().await?;

    let restarted = tokio::time::timeout(
        PATIENCE,
        child.wait_generation(RestartGeneration::FIRST.next()),
    )
    .await
    .expect("the one restart the allowance permits must happen")?;
    restarted.stop().await?;

    let status = tokio::time::timeout(PATIENCE, child.wait_for(|s| s.state().is_terminal()))
        .await
        .expect("an exhausted child must reach a terminal state, not sit pending")?;

    Ok(status.state())
}

/// Whether the supervisor answers a ping, which is whether it is still running.
async fn supervisor_still_answers(parent: &ActorHandle, pings: &Arc<AtomicUsize>) -> bool {
    let before = pings.load(Ordering::SeqCst);
    let _ = tokio::time::timeout(PATIENCE, parent.send(Ping)).await;

    for _ in 0..200 {
        if pings.load(Ordering::SeqCst) > before {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Polls until `counter` reaches `expected`, or gives up and returns what it saw.
async fn wait_for_count(counter: &Arc<AtomicUsize>, expected: usize) -> usize {
    for _ in 0..300 {
        let seen = counter.load(Ordering::SeqCst);
        if seen >= expected {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    counter.load(Ordering::SeqCst)
}

#[acton_test]
async fn notify_parent_tells_the_grandparent_and_keeps_the_supervisor_running() -> anyhow::Result<()>
{
    // The default, and the reason it is the default: in this framework a
    // supervisor is usually also a working actor, so one child that cannot be
    // kept alive must not take down unrelated work.
    let mut chain = escalating_chain(Escalation::NotifyParent).await?;

    assert_eq!(
        exhaust_the_allowance(&mut chain.child).await?,
        SupervisionState::Escalated
    );

    assert_eq!(
        wait_for_count(&chain.escalations, 1).await,
        1,
        "the supervisor's own parent is told that it gave up"
    );

    // The half that makes this test about NotifyParent rather than about
    // escalation in general.
    assert!(
        supervisor_still_answers(&chain.parent, &chain.pings).await,
        "the supervisor keeps running and keeps taking messages"
    );

    chain.runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn stop_supervisor_stops_the_supervisor_itself() -> anyhow::Result<()> {
    // The Erlang/OTP behaviour, and the only thing that distinguishes it from
    // the default: the supervisor goes away. Everything about the *child* is
    // identical under both policies, so a test that only looked at the child
    // could not tell them apart.
    let mut chain = escalating_chain(Escalation::StopSupervisor).await?;

    assert_eq!(
        exhaust_the_allowance(&mut chain.child).await?,
        SupervisionState::Escalated
    );

    // The supervisor stops rather than carrying on. One check is enough:
    // `supervisor_still_answers` already waits out its own patience before
    // concluding nothing answered, and the escalation was recorded before the
    // child's status settled above.
    assert!(
        !supervisor_still_answers(&chain.parent, &chain.pings).await,
        "StopSupervisor must stop the supervisor, not merely log about it"
    );

    chain.runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_supervisor_with_no_parent_escalates_without_complaining() -> anyhow::Result<()> {
    // The top of a tree has nobody to notify. That is the normal case for a
    // root supervisor, not a failure, and it must not stop the escalation
    // bookkeeping from completing — a child left waiting on a status that never
    // settles is the failure this guards against.
    let mut runtime = ActonApp::launch_async().await;

    let config = ActorConfig::new_with_name("root")?.with_escalation(Escalation::NotifyParent);
    let mut parent = runtime.new_actor_with_config::<Parent>(config);

    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();
    parent.mutate_on::<HireWorker>(move |actor, _ctx| {
        let child_config =
            ActorConfig::for_supervised_child("worker", actor.handle().clone(), None)
                .expect("a name plus a live parent is a valid child configuration")
                .with_restart_policy(RestartPolicy::Permanent)
                .with_restart_limiter(one_restart_only());

        let child =
            actor.supervise_deferred(child_config, |worker: &mut ManagedActor<Idle, Worker>| {
                worker.mutate_on::<Ping>(|_worker, _ctx| Reply::ready());
            });
        let _ = registered.send(child);
        Reply::ready()
    });
    let parent = parent.start().await;

    parent.send(HireWorker).await;
    let mut child = tokio::time::timeout(PATIENCE, registrations.recv())
        .await
        .expect("the supervisor must answer a hire")
        .expect("the channel is open")
        .expect("the first child of a name is accepted");

    assert_eq!(
        exhaust_the_allowance(&mut child).await?,
        SupervisionState::Escalated,
        "a root supervisor still settles the child rather than waiting on a parent it has not got"
    );

    runtime.shutdown_all().await?;
    Ok(())
}
