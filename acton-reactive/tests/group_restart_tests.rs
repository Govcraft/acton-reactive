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

//! Group restarts: `OneForAll` and `RestForOne` carried out end to end.
//!
//! # Two traps these tests are shaped to avoid
//!
//! **`ActorHandle: PartialEq` compares the `Ern` only, and a restart keeps the
//! `Ern`.** So `assert_ne!(before, after)` on two handles passes whether or not
//! anything was restarted, and proves nothing. Liveness here is proved by
//! routing a [`Ping`] to a handle and asserting a counter *owned by that
//! incarnation* moved — a dead mailbox does not answer, and a stale handle
//! increments the counter of an instance that is gone.
//!
//! **A test that only asserts what changed cannot tell `RestForOne` from
//! `OneForAll`.** Every test below also asserts what stayed put, which is what
//! makes the strategy under test the thing being measured.
//!
//! Most of these fail by **hanging** rather than by asserting — a restart that
//! never happens leaves a caller waiting on a status that never changes — so
//! every wait is wrapped in an explicit timeout that fails loudly instead.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a coffee break.
const PATIENCE: Duration = Duration::from_secs(5);

/// The three children every test here supervises, in start order.
const NAMES: [&str; 3] = ["alpha", "bravo", "charlie"];

#[acton_actor]
struct Parent;

#[acton_actor]
struct Worker;

/// Asks the supervisor to hire every child in [`NAMES`], in order.
#[acton_message]
struct HireWorkers;

/// Routed to a child to prove that incarnation is alive and taking messages.
#[acton_message]
struct Ping;

/// What each child did and when, in the order it happened.
///
/// The ordering rules are the entire content of a group restart — stop in
/// reverse start order, start in start order — so a test that does not observe
/// the order is not testing the strategy.
///
/// Every event is kept twice over: appended to `recorded`, which is what the
/// ordering assertions read, and announced on `announce`, which is how a caller
/// *waits* for an event rather than sleeping and hoping it has landed. The
/// append happens before the announcement, so a caller that has received `n`
/// announcements is guaranteed to find those `n` events already in `recorded`.
#[derive(Clone)]
struct EventLog {
    recorded: Arc<Mutex<Vec<String>>>,
    announce: tokio::sync::mpsc::UnboundedSender<String>,
}

/// The announcement of every event, in the order the events were recorded.
type EventStream = tokio::sync::mpsc::UnboundedReceiver<String>;

impl EventLog {
    fn new() -> (Self, EventStream) {
        let (announce, stream) = tokio::sync::mpsc::unbounded_channel();
        let log = Self {
            recorded: Arc::new(Mutex::new(Vec::new())),
            announce,
        };
        (log, stream)
    }

    fn record(&self, event: String) {
        self.recorded
            .lock()
            .expect("the log mutex is never poisoned")
            .push(event.clone());
        // A test that is not watching the stream has dropped the receiver. That
        // is not a failure; it just means nobody is waiting on this event.
        let _ = self.announce.send(event);
    }

    /// The events recorded since `from`, which is where the failure was injected.
    fn since(&self, from: usize) -> Vec<String> {
        self.recorded
            .lock()
            .expect("the log mutex is never poisoned")
            .iter()
            .skip(from)
            .cloned()
            .collect()
    }

    fn len(&self) -> usize {
        self.recorded
            .lock()
            .expect("the log mutex is never poisoned")
            .len()
    }
}

/// Waits for the next recorded event, failing loudly rather than hanging.
async fn next_event(stream: &mut EventStream) -> String {
    tokio::time::timeout(PATIENCE, stream.recv())
        .await
        .expect("a lifecycle event must arrive rather than the test waiting out the clock")
        .expect("the log's sender outlives the actors that write to it")
}

/// Waits for the next `count` events, in the order they were recorded.
async fn next_events(stream: &mut EventStream, count: usize) -> Vec<String> {
    let mut events = Vec::with_capacity(count);
    for _ in 0..count {
        events.push(next_event(stream).await);
    }
    events
}

/// Per-child counters, indexed the same as [`NAMES`].
#[derive(Clone)]
struct Counters {
    /// How many incarnations of each child have been built.
    ///
    /// The blueprint runs once per incarnation, so this is the only thing that
    /// distinguishes a replacement from the original: a restart deliberately
    /// keeps the child's identifier.
    builds: [Arc<AtomicUsize>; 3],
    /// How many pings each child's *current* incarnation has answered.
    ///
    /// Reset by construction rather than by hand: each incarnation gets a fresh
    /// counter, so a count that moves is a count belonging to something alive.
    pings: [Arc<AtomicUsize>; 3],
}

impl Counters {
    fn new() -> Self {
        Self {
            builds: std::array::from_fn(|_| Arc::new(AtomicUsize::new(0))),
            pings: std::array::from_fn(|_| Arc::new(AtomicUsize::new(0))),
        }
    }

    fn builds(&self, index: usize) -> usize {
        self.builds[index].load(Ordering::SeqCst)
    }

    fn pings(&self, index: usize) -> usize {
        self.pings[index].load(Ordering::SeqCst)
    }
}

/// Restart settings that make a test quick and its arithmetic obvious.
///
/// The multiplier is 1.0 so a test asserting *which* incarnation is running
/// does not also have to model exponential growth.
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

/// A supervisor that hires all three children when asked, in [`NAMES`] order.
///
/// Start order is what `RestForOne` slices on, so the registrations are issued
/// from one handler turn rather than three messages that could interleave.
fn supervising_parent(
    runtime: &mut ActorRuntime,
    strategy: SupervisionStrategy,
    counters: &Counters,
    log: &EventLog,
    registered: tokio::sync::mpsc::UnboundedSender<SupervisedChild>,
) -> ManagedActor<Idle, Parent> {
    let config = ActorConfig::new_with_name("pool")
        .expect("'pool' is a valid root name")
        .with_supervision_strategy(strategy);
    let mut parent = runtime.new_actor_with_config::<Parent>(config);

    let counters = counters.clone();
    let log = log.clone();
    parent.mutate_on::<HireWorkers>(move |actor, _ctx| {
        for (index, name) in NAMES.iter().enumerate() {
            let child_config =
                ActorConfig::for_supervised_child(*name, actor.handle().clone(), None)
                    .expect("a name plus a live parent is a valid child configuration")
                    .with_restart_policy(RestartPolicy::Permanent)
                    .with_restart_limiter(brisk_limiter(5));

            let child = actor
                .supervise_deferred(child_config, worker_blueprint(index, &counters, &log))
                .expect("the first registration of a name succeeds");
            let _ = registered.send(child);
        }
        Reply::ready()
    });

    parent
}

/// A child that records its own comings and goings and answers pings.
fn worker_blueprint(
    index: usize,
    counters: &Counters,
    log: &EventLog,
) -> impl Fn(&mut ManagedActor<Idle, Worker>) + Clone + Send + Sync + 'static {
    let builds = Arc::clone(&counters.builds[index]);
    let pings = Arc::clone(&counters.pings[index]);
    let log = log.clone();
    let name = NAMES[index];

    move |actor: &mut ManagedActor<Idle, Worker>| {
        builds.fetch_add(1, Ordering::SeqCst);
        // Each incarnation counts its own pings, so a count that moves belongs
        // to something that is alive now rather than to a handle held over.
        pings.store(0, Ordering::SeqCst);

        let answered = Arc::clone(&pings);
        actor.mutate_on::<Ping>(move |_actor, _ctx| {
            answered.fetch_add(1, Ordering::SeqCst);
            Reply::ready()
        });

        let started = log.clone();
        actor.after_start(move |_actor| {
            started.record(format!("start:{name}"));
            Reply::ready()
        });

        let stopped = log.clone();
        actor.after_stop(move |_actor| {
            stopped.record(format!("stop:{name}"));
            Reply::ready()
        });
    }
}

/// Hires all three children and waits until each is running.
async fn hire_all(
    parent: &ActorHandle,
    registrations: &mut tokio::sync::mpsc::UnboundedReceiver<SupervisedChild>,
) -> anyhow::Result<Vec<(SupervisedChild, ActorHandle)>> {
    parent.send(HireWorkers).await;

    let mut children = Vec::with_capacity(NAMES.len());
    for name in NAMES {
        let mut child = tokio::time::timeout(PATIENCE, registrations.recv())
            .await
            .unwrap_or_else(|_| panic!("the supervisor must report hiring {name}"))
            .expect("the channel is open");
        let handle = tokio::time::timeout(PATIENCE, child.wait_running())
            .await
            .unwrap_or_else(|_| panic!("the first start of {name} must land"))?;
        children.push((child, handle));
    }

    Ok(children)
}

/// Sends a ping and returns whether the counter for that child moved.
///
/// The only honest liveness check available: a restart keeps the `Ern`, so
/// comparing handles proves nothing, and a handle whose actor is gone accepts
/// the send without anybody answering it.
async fn answers_ping(handle: &ActorHandle, counter: &Arc<AtomicUsize>) -> bool {
    let before = counter.load(Ordering::SeqCst);
    let _ = tokio::time::timeout(PATIENCE, handle.send(Ping)).await;

    for _ in 0..200 {
        if counter.load(Ordering::SeqCst) > before {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

/// Waits until every named child has been built `expected` times.
///
/// Polls rather than sleeping a fixed interval so a slow machine does not turn
/// a passing test into a flaky one, and so a failure reports the counts.
async fn wait_for_builds(counters: &Counters, expected: [usize; 3]) -> [usize; 3] {
    for _ in 0..500 {
        let seen = [counters.builds(0), counters.builds(1), counters.builds(2)];
        if seen == expected {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    [counters.builds(0), counters.builds(1), counters.builds(2)]
}

/// The generation a child reaches after `restarts` restarts.
fn generation_after(restarts: u32) -> RestartGeneration {
    let mut generation = RestartGeneration::FIRST;
    for _ in 0..restarts {
        generation = generation.next();
    }
    generation
}

/// The position of `event` in `events`, or a failure naming what was recorded.
fn position(events: &[String], event: &str) -> usize {
    events
        .iter()
        .position(|seen| seen == event)
        .unwrap_or_else(|| panic!("'{event}' never happened; the log holds {events:?}"))
}

#[acton_test]
async fn one_for_all_restarts_every_child_when_one_of_them_fails() -> anyhow::Result<()> {
    // **Fails by hanging** without the group sequencer: only the child that
    // failed comes back, so `alpha` and `charlie` are never rebuilt and the
    // build counts never reach 2.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();
    let counters = Counters::new();
    // Held rather than dropped so the log keeps its announcements; this test
    // reads the recorded order directly instead of waiting on them.
    let (log, _stream) = EventLog::new();

    let parent = supervising_parent(
        &mut runtime,
        SupervisionStrategy::OneForAll,
        &counters,
        &log,
        registered,
    )
    .start()
    .await;

    let children = hire_all(&parent, &mut registrations).await?;
    assert_eq!(
        [counters.builds(0), counters.builds(1), counters.builds(2)],
        [1, 1, 1],
        "each child is built once before anything fails"
    );

    // Take down the middle child. `Permanent` restarts on a normal
    // termination, which is what makes an explicit stop a stand-in for a crash.
    children[1].1.stop().await?;

    let builds = wait_for_builds(&counters, [2, 2, 2]).await;
    assert_eq!(
        builds,
        [2, 2, 2],
        "OneForAll rebuilds the whole set, not just the child that failed"
    );

    // Every child is back at generation 1 and reachable, proved by routing a
    // ping to the incarnation the supervisor is now holding.
    for (index, name) in NAMES.iter().enumerate() {
        let mut child = children[index].0.clone();
        let fresh = tokio::time::timeout(
            PATIENCE,
            child.wait_generation(RestartGeneration::FIRST.next()),
        )
        .await
        .unwrap_or_else(|_| panic!("{name} must come back"))?;

        assert!(
            answers_ping(&fresh, &counters.pings[index]).await,
            "{name}'s replacement must be a live actor rather than a record"
        );
    }

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn one_for_all_stops_in_reverse_start_order_and_stops_everything_before_starting_anything(
) -> anyhow::Result<()> {
    // The ordering *is* the strategy. A group restart that stopped children in
    // start order would tear down a dependency before the children that need
    // it, which is the failure `OneForAll` exists to prevent.
    //
    // `bravo` is the child that fails, so the supervisor stops `charlie` and
    // then `alpha`, and only then rebuilds.
    //
    // # What this deliberately does not assert
    //
    // That the replacements *finish* starting in start order. The supervisor
    // asks for them in start order, but each start runs on its own task —
    // that concurrency is the whole point of the start path, because a child's
    // `before_start` is user code of unbounded duration and awaiting it would
    // stop the supervisor taking messages. So completion order is genuinely not
    // guaranteed, and an assertion on it would be a flaky test dressed up as a
    // specification. What *is* guaranteed, and asserted here, is that every
    // stop precedes every start.
    //
    // # Why this waits on events rather than on counters or a clock
    //
    // Both waits below are for the events themselves, because every cheaper
    // proxy is wrong here, and was:
    //
    // - **`wait_running()` does not mean `after_start` has run.** It resolves
    //   when the *supervisor* publishes `Running`, which it does as soon as the
    //   child's task is spawned; `after_start` runs on that task and can still
    //   be pending. Snapshotting the log's length after `hire_all` therefore
    //   left opening `start:` events to be recorded *after* the snapshot, where
    //   they read as starts belonging to the restart. `first_start` then pointed
    //   at an event from before the failure was even injected and the ordering
    //   assertion failed against a system that had behaved perfectly.
    // - **`builds` does not mean `after_start` has run either.** The blueprint
    //   increments it, and the blueprint runs before the actor is started.
    // - **A count of events is not the events.** The previous wait stopped at
    //   five of the six, so an assertion could run against a log still missing
    //   a start.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();
    let counters = Counters::new();
    let (log, mut stream) = EventLog::new();

    let parent = supervising_parent(
        &mut runtime,
        SupervisionStrategy::OneForAll,
        &counters,
        &log,
        registered,
    )
    .start()
    .await;

    let children = hire_all(&parent, &mut registrations).await?;

    // Drain the three opening starts, so that everything read below genuinely
    // belongs to the restart rather than to the hiring that preceded it.
    let mut opening = next_events(&mut stream, NAMES.len()).await;
    opening.sort();
    assert_eq!(
        opening,
        ["start:alpha", "start:bravo", "start:charlie"],
        "the opening events are the three children starting, once each"
    );

    children[1].1.stop().await?;

    // A group restart is exactly six events: every child stops, then every child
    // starts. Waiting for the sixth is what makes the assertions below read a
    // finished restart rather than one caught in the middle.
    let events = next_events(&mut stream, 2 * NAMES.len()).await;

    // Deterministic now rather than polled: every child's `after_start` has run,
    // and a blueprint always runs before the `after_start` of the incarnation it
    // built, so the build counts cannot still be settling.
    assert_eq!(
        [counters.builds(0), counters.builds(1), counters.builds(2)],
        [2, 2, 2],
        "the whole group was rebuilt exactly once: {events:?}"
    );

    // Each child stopped once and started once, and nothing else happened.
    // Without this, "stops everything" rested entirely on `position` panicking
    // rather than on anything asserted, and a group that stopped a child twice
    // or started a fourth would have passed.
    let mut seen: Vec<&str> = events.iter().map(String::as_str).collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        [
            "start:alpha",
            "start:bravo",
            "start:charlie",
            "stop:alpha",
            "stop:bravo",
            "stop:charlie",
        ],
        "a group restart stops each child exactly once and starts each exactly once: {events:?}"
    );

    // `bravo` is excluded from the sequencer's stop list — it is the child that
    // failed, and it is already down — so the ordering the strategy controls is
    // charlie before alpha.
    assert!(
        position(&events, "stop:charlie") < position(&events, "stop:alpha"),
        "siblings stop in reverse start order, so charlie goes before alpha: {events:?}"
    );

    // Everything stops before anything starts. Asserted against the *last*
    // stop and the *first* start rather than one pair, so a single child
    // slipping back up early is caught wherever in the group it sits.
    let last_stop = ["stop:alpha", "stop:bravo", "stop:charlie"]
        .iter()
        .map(|event| position(&events, event))
        .max()
        .expect("three stops");
    let first_start = ["start:alpha", "start:bravo", "start:charlie"]
        .iter()
        .map(|event| position(&events, event))
        .min()
        .expect("three starts");
    assert!(
        last_stop < first_start,
        "the group is fully down before any of it comes back: {events:?}"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn rest_for_one_leaves_the_children_started_before_the_failure_untouched(
) -> anyhow::Result<()> {
    // The assertion that separates `RestForOne` from `OneForAll`. Without the
    // "alpha was untouched" half, a mutation turning `RestForOne` into
    // `OneForAll` passes this test unchanged: bravo and charlie come back
    // either way.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();
    let counters = Counters::new();
    // Held rather than dropped so the log keeps its announcements; this test
    // reads the recorded order directly instead of waiting on them.
    let (log, _stream) = EventLog::new();

    let parent = supervising_parent(
        &mut runtime,
        SupervisionStrategy::RestForOne,
        &counters,
        &log,
        registered,
    )
    .start()
    .await;

    let children = hire_all(&parent, &mut registrations).await?;

    // Prove alpha is alive *before* the failure, so the check afterwards is a
    // comparison rather than an unanchored assertion.
    assert!(answers_ping(&children[0].1, &counters.pings[0]).await);
    let alpha_pings_before = counters.pings(0);

    children[1].1.stop().await?;

    let builds = wait_for_builds(&counters, [1, 2, 2]).await;
    assert_eq!(
        builds,
        [1, 2, 2],
        "RestForOne rebuilds from the failure onwards and no further back"
    );

    for index in [1usize, 2] {
        let mut child = children[index].0.clone();
        let fresh = tokio::time::timeout(
            PATIENCE,
            child.wait_generation(RestartGeneration::FIRST.next()),
        )
        .await
        .unwrap_or_else(|_| panic!("{} must come back", NAMES[index]))?;
        assert!(
            answers_ping(&fresh, &counters.pings[index]).await,
            "{}'s replacement must be a live actor",
            NAMES[index]
        );
    }

    // ---- and now the half that makes this test about RestForOne ----

    assert_eq!(
        children[0].0.clone().status().generation(),
        RestartGeneration::FIRST,
        "alpha was never restarted, so it is still on its first incarnation"
    );
    assert_eq!(
        children[0].0.clone().status().state(),
        SupervisionState::Running,
        "alpha never left Running"
    );
    assert_eq!(
        counters.pings(0),
        alpha_pings_before,
        "alpha was never rebuilt, so its ping counter was never reset"
    );
    assert!(
        answers_ping(&children[0].1, &counters.pings[0]).await,
        "and the original alpha is still the live one, answering on its original handle"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_group_restart_is_charged_to_the_child_that_failed_and_not_to_its_siblings(
) -> anyhow::Result<()> {
    // A group restart is one supervisory event, so it costs one restart. Charge
    // it per child and three children failing in rotation exhaust an allowance
    // of five in under two rounds, and a healthy supervisor escalates children
    // that never failed.
    //
    // Measured through the allowance rather than a counter: with `max_restarts:
    // 2`, a per-child charge gives every sibling two group restarts' worth of
    // credit and the third round escalates. Charged once, each child has only
    // spent what it caused.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();
    let counters = Counters::new();
    // Held rather than dropped so the log keeps its announcements; this test
    // reads the recorded order directly instead of waiting on them.
    let (log, _stream) = EventLog::new();

    let parent = supervising_parent(
        &mut runtime,
        SupervisionStrategy::OneForAll,
        &counters,
        &log,
        registered,
    )
    .start()
    .await;

    let children = hire_all(&parent, &mut registrations).await?;

    // Fail `bravo` three times. Only bravo is ever the cause, so only bravo's
    // allowance should be spent; alpha and charlie are stopped and restarted
    // each round without being charged for it.
    let mut current = children[1].1.clone();
    for round in 1..=3u32 {
        current.stop().await?;

        let expected = usize::try_from(round).expect("a small round number") + 1;
        assert_eq!(
            wait_for_builds(&counters, [expected, expected, expected]).await,
            [expected, expected, expected],
            "round {round} must rebuild the whole group"
        );

        let mut child = children[1].0.clone();
        current = tokio::time::timeout(PATIENCE, child.wait_generation(generation_after(round)))
            .await
            .unwrap_or_else(|_| panic!("bravo must come back for round {round}"))?;
    }

    // The measurement. Each child publishes its own limiter's count, so this
    // reads the allowance directly rather than inferring it from behaviour.
    assert_eq!(
        children[1].0.status().restarts_in_window(),
        3,
        "bravo caused three group restarts and is charged for three"
    );
    for index in [0usize, 2] {
        assert_eq!(
            children[index].0.status().restarts_in_window(),
            0,
            "{} was stopped and rebuilt three times without ever failing, and a group \
             restart is one supervisory event rather than one per child",
            NAMES[index]
        );
        assert_eq!(
            children[index].0.status().state(),
            SupervisionState::Running,
            "{} is still up",
            NAMES[index]
        );
    }

    runtime.shutdown_all().await?;
    Ok(())
}

#[acton_test]
async fn a_group_restart_stops_a_child_it_cannot_bring_back_and_leaves_it_down(
) -> anyhow::Result<()> {
    // The genuinely surprising rule, and the one a mutation is most likely to
    // get wrong in the *forgiving* direction: treating `then_restart: false` as
    // `true` resurrects a child the supervisor has no recipe for.
    //
    // "Cannot bring back" means **no blueprint**, not a restrictive
    // `RestartPolicy`. A policy is consulted about the child that *failed*; the
    // planner filters siblings on whether the supervisor holds a blueprint at
    // all. So the child that stands for this rule is one adopted through the
    // legacy `supervise()` path — which is also the case that matters in the
    // field, because it is the only one an existing program can hit.
    let mut runtime = ActonApp::launch_async().await;
    let (registered, mut registrations) = tokio::sync::mpsc::unbounded_channel();
    let counters = Counters::new();
    // Held rather than dropped so the log keeps its announcements; this test
    // reads the recorded order directly instead of waiting on them.
    let (log, _stream) = EventLog::new();

    let parent = supervising_parent(
        &mut runtime,
        SupervisionStrategy::OneForAll,
        &counters,
        &log,
        registered,
    )
    .start()
    .await;

    let children = hire_all(&parent, &mut registrations).await?;

    // A fourth child the supervisor adopts rather than builds: it holds a
    // handle to it but no recipe for making another.
    let adopted_builds = Arc::new(AtomicUsize::new(0));
    let adopted_config = ActorConfig::for_supervised_child("adopted", parent.clone(), None)?
        .with_restart_policy(RestartPolicy::Permanent);
    let mut adopted = runtime.new_actor_with_config::<Worker>(adopted_config);
    {
        let builds = Arc::clone(&adopted_builds);
        let log = log.clone();
        builds.fetch_add(1, Ordering::SeqCst);
        adopted.after_stop(move |_actor| {
            log.record("stop:adopted".to_string());
            Reply::ready()
        });
    }
    parent.supervise(adopted).await?;

    let injected_at = log.len();

    children[1].1.stop().await?;

    // The three engine-managed children come back.
    assert_eq!(
        wait_for_builds(&counters, [2, 2, 2]).await,
        [2, 2, 2],
        "the children the supervisor can rebuild are rebuilt"
    );

    // The adopted one was stopped for consistency and stays down.
    for _ in 0..200 {
        if log
            .since(injected_at)
            .iter()
            .any(|event| event == "stop:adopted")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let events = log.since(injected_at);

    assert!(
        events.iter().any(|event| event == "stop:adopted"),
        "a sibling the group cannot recreate is still brought down, or the freshly \
         restarted set runs against inconsistent state: {events:?}"
    );
    assert_eq!(
        adopted_builds.load(Ordering::SeqCst),
        1,
        "and it never comes back: the supervisor has no blueprint to build it from"
    );

    runtime.shutdown_all().await?;
    Ok(())
}
