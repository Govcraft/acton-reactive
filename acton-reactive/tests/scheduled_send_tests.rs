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

//! What `send_after`, `send_at`, and `send_every` promise.
//!
//! Two house rules, inherited from `ask_tests.rs`:
//!
//! * **No sleeps.** Time is a [`ManualClock`] the test drives, and progress is
//!   observed through [`ScheduledSend::wait_for_deliveries`] and
//!   [`ScheduledSend::outcome`]. The one test that uses the real clock is
//!   testing the real clock.
//! * **Every await has a deadline**, so a regression that hangs shows up as a
//!   failure rather than a broken run.
//!
//! # Why the negative assertions here are not races
//!
//! Several tests assert that a message has *not* been delivered. That is sound
//! rather than lucky because a [`ManualClock`] timer resolves only when the
//! observed instant reaches its deadline: while the clock sits short of one,
//! the delivery count cannot move however the runtime schedules things. And
//! because a re-armed tick is always strictly in the future, a count read after
//! `wait_for_deliveries` returns is final until the test advances the clock
//! again.

use std::sync::Arc;
use std::time::Duration;

use acton_reactive::prelude::*;
use acton_test::prelude::*;

/// Long enough to be decisive, short enough that a hang is not a break.
const PATIENCE: Duration = Duration::from_secs(5);

/// The interval every repeating test is built around.
const STEP: Duration = Duration::from_secs(10);

/// Counts what it has been sent, and remembers the order it arrived in.
#[acton_actor]
struct Tally {
    ticks: usize,
    notes: Vec<String>,
}

/// The message scheduled by nearly every test here.
#[acton_message]
struct Tick;

/// A message that says which one it is, for asserting arrival order.
#[acton_message]
struct Note {
    label: String,
}

/// Asks the actor what it has actually received.
#[acton_message]
struct GetTally;

/// The actor's answer.
#[acton_message]
#[derive(PartialEq, Eq)]
struct Count {
    ticks: usize,
    notes: Vec<String>,
}

impl Request for GetTally {
    type Response = Count;
}

/// One interval, as an [`Interval`].
const fn step() -> Interval {
    Interval::new(STEP).expect("ten seconds is not zero")
}

/// A fraction of an interval, for separating the two cadences.
fn part_of_step(numerator: u32, denominator: u32) -> Duration {
    STEP * numerator / denominator
}

/// Starts an actor that records everything it is sent.
async fn start_tally(runtime: &mut ActorRuntime) -> ActorHandle {
    let mut actor = runtime.new_actor::<Tally>();

    actor.mutate_on::<Tick>(|actor, _| {
        actor.model.ticks += 1;
        Reply::ready()
    });

    actor.mutate_on::<Note>(|actor, context| {
        actor.model.notes.push(context.message().label.clone());
        Reply::ready()
    });

    actor.act_on::<GetTally>(|actor, context| {
        let reply = context.reply_envelope();
        let answer = Count {
            ticks: actor.model.ticks,
            notes: actor.model.notes.clone(),
        };
        Reply::pending(async move { reply.send(answer).await })
    });

    actor.start().await
}

/// Asks the actor what it received, with a deadline.
async fn tally(handle: &ActorHandle) -> anyhow::Result<Count> {
    let answer = tokio::time::timeout(PATIENCE, handle.ask(GetTally))
        .await
        .expect("the tally must resolve, not hang")?;
    Ok(answer)
}

/// Waits for the schedule to settle, with a deadline.
async fn settled(scheduled: &ScheduledSend) -> ScheduledSendOutcome {
    tokio::time::timeout(PATIENCE, scheduled.outcome())
        .await
        .expect("a scheduled send must always settle, never hang")
}

/// Waits for `at_least` deliveries, with a deadline.
async fn delivered(scheduled: &ScheduledSend, at_least: u64) -> u64 {
    tokio::time::timeout(PATIENCE, scheduled.wait_for_deliveries(at_least))
        .await
        .expect("the deliveries barrier must resolve, not hang")
}

// --- one-shot -------------------------------------------------------------

/// The whole promise of `send_after` in one test: nothing before the deadline,
/// the message after it, and the actor really did handle it.
#[acton_test]
async fn a_delayed_send_waits_for_its_deadline_and_then_arrives() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let scheduled = handle.with_clock(clock.clone()).send_after(Tick, STEP);

    assert_eq!(
        clock.armed(),
        1,
        "the timer must be armed by the time `send_after` returns"
    );
    assert!(
        scheduled.settled().is_none(),
        "a schedule that has not come due has no outcome yet"
    );

    clock.advance(part_of_step(9, 10));
    assert_eq!(
        scheduled.deliveries(),
        0,
        "nine tenths of the way to the deadline is short of the deadline"
    );

    clock.advance(part_of_step(1, 10));

    assert_eq!(settled(&scheduled).await, ScheduledSendOutcome::Delivered);
    assert_eq!(
        tally(&handle).await?.ticks,
        1,
        "the actor must have handled the message, not merely been sent it"
    );

    runtime.shutdown_all().await
}

/// Cancelling before the deadline must stop the send outright — not deliver it
/// and report `Cancelled`, which would be the worst of both answers.
#[acton_test]
async fn cancelling_before_the_deadline_sends_nothing() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let scheduled = handle.with_clock(clock.clone()).send_after(Tick, STEP);
    scheduled.cancel();

    assert_eq!(settled(&scheduled).await, ScheduledSendOutcome::Cancelled);

    // The schedule has settled, so the timer task has already returned. Moving
    // the clock far past the original deadline cannot resurrect it.
    clock.advance(STEP * 100);

    assert_eq!(
        tally(&handle).await?.ticks,
        0,
        "a cancelled send must never reach the actor"
    );

    runtime.shutdown_all().await
}

/// Cancelling something that has already happened cannot unhappen it. A
/// `cancel()` racing a delivery must not rewrite a settled outcome.
#[acton_test]
async fn cancelling_after_delivery_leaves_the_outcome_alone() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let scheduled = handle.with_clock(clock.clone()).send_after(Tick, STEP);
    clock.advance(STEP);
    assert_eq!(settled(&scheduled).await, ScheduledSendOutcome::Delivered);

    scheduled.cancel();

    assert_eq!(
        scheduled.settled(),
        Some(ScheduledSendOutcome::Delivered),
        "a delivered send stays delivered"
    );
    assert_eq!(tally(&handle).await?.ticks, 1);

    runtime.shutdown_all().await
}

/// A schedule outliving its target must end, and say so. Nothing else could
/// report this: the message was neither sent nor cancelled.
#[acton_test]
async fn stopping_the_actor_abandons_a_pending_send() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let scheduled = handle
        .with_clock(clock.clone())
        .send_after(Tick, STEP * 100);

    tokio::time::timeout(PATIENCE, handle.stop())
        .await
        .expect("stopping must not wait for a pending schedule")?;

    assert_eq!(settled(&scheduled).await, ScheduledSendOutcome::Abandoned);

    runtime.shutdown_all().await
}

/// A deadline that has already passed sends promptly rather than being
/// discarded, so a schedule restored from persisted state does not silently
/// lose whatever fell due while the process was down.
#[acton_test]
async fn a_deadline_already_in_the_past_sends_at_once() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let long_past = clock
        .now()
        .checked_sub(STEP * 100)
        .expect("a long time ago");

    let scheduled = handle.with_clock(clock.clone()).send_at(Tick, long_past);

    assert_eq!(settled(&scheduled).await, ScheduledSendOutcome::Delivered);
    assert_eq!(tally(&handle).await?.ticks, 1);

    runtime.shutdown_all().await
}

/// Deadlines decide the order, not the order the schedules were created in.
#[acton_test]
async fn messages_arrive_in_deadline_order_not_call_order() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;
    let scheduling = handle.with_clock(clock.clone());

    // Deliberately scheduled the wrong way round.
    let late = scheduling.send_after(
        Note {
            label: "late".to_owned(),
        },
        STEP * 2,
    );
    let early = scheduling.send_after(
        Note {
            label: "early".to_owned(),
        },
        STEP,
    );

    clock.advance(STEP);
    assert_eq!(delivered(&early, 1).await, 1);
    assert_eq!(
        late.deliveries(),
        0,
        "the later deadline has not been reached"
    );

    clock.advance(STEP);
    assert_eq!(delivered(&late, 1).await, 1);

    assert_eq!(
        tally(&handle).await?.notes,
        vec!["early".to_owned(), "late".to_owned()],
    );

    runtime.shutdown_all().await
}

/// Without `with_clock`, scheduling uses real time. Everything else here drives
/// a `ManualClock`, so this is the only test that would notice if the default
/// clock were wired up wrongly.
#[acton_test]
async fn the_default_clock_is_real_time() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_tally(&mut runtime).await;

    let scheduled = handle.send_after(Tick, Duration::from_millis(10));

    assert_eq!(settled(&scheduled).await, ScheduledSendOutcome::Delivered);
    assert_eq!(tally(&handle).await?.ticks, 1);

    runtime.shutdown_all().await
}

/// `with_clock` changes how a handle schedules and nothing else — same actor,
/// same identity, same inbox.
#[acton_test]
async fn with_clock_still_addresses_the_same_actor() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_tally(&mut runtime).await;

    let scheduling = handle.with_clock(Arc::new(ManualClock::new()));

    assert_eq!(scheduling, handle, "the handle still names the same actor");

    scheduling.send(Tick).await;
    assert_eq!(
        tally(&handle).await?.ticks,
        1,
        "an ordinary send through the re-clocked handle reaches the same inbox"
    );

    runtime.shutdown_all().await
}

// --- repeating ------------------------------------------------------------

/// The test that guards the two mistakes a repeating timer invites, both of
/// which are silent: parking the task on the actor's `TaskTracker` would make
/// `stop` wait for a schedule that never ends, and selecting on the handle's
/// cancellation token — which nothing in the crate ever cancels — would leave
/// the task running forever after the actor is gone. Either shows up here as a
/// timeout.
#[acton_test]
async fn stopping_the_actor_ends_a_repeating_schedule_promptly() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let handle = start_tally(&mut runtime).await;

    // Real time, and an interval far longer than the test's patience: nothing
    // here can end the schedule except the actor going away.
    let hourly = Interval::from_secs(3600).expect("an hour is not zero");
    let ticks = handle.send_every(Tick, hourly, Cadence::FixedRate);

    tokio::time::timeout(PATIENCE, handle.stop())
        .await
        .expect("stop must not wait for a repeating schedule")?;

    assert_eq!(settled(&ticks).await, ScheduledSendOutcome::Abandoned);

    runtime.shutdown_all().await
}

/// The first tick waits a whole interval. Firing one immediately would make
/// `send_every` and `send_after` disagree about what an interval means.
#[acton_test]
async fn the_first_tick_waits_a_whole_interval() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let ticks = handle
        .with_clock(clock.clone())
        .send_every(Tick, step(), Cadence::FixedRate);

    assert_eq!(ticks.deliveries(), 0, "no tick at arming time");

    clock.advance(part_of_step(9, 10));
    assert_eq!(
        ticks.deliveries(),
        0,
        "and none nine tenths of the way there"
    );

    clock.advance(part_of_step(1, 10));
    assert_eq!(delivered(&ticks, 1).await, 1);

    ticks.cancel();
    runtime.shutdown_all().await
}

/// A repeating schedule re-arms rather than settling after its first delivery.
#[acton_test]
async fn a_repeating_schedule_keeps_ticking() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let ticks = handle
        .with_clock(clock.clone())
        .send_every(Tick, step(), Cadence::FixedRate);

    for expected in 1..=5_u64 {
        clock.advance(STEP);
        assert_eq!(delivered(&ticks, expected).await, expected);
    }

    assert!(
        !ticks.is_settled(),
        "a repeating schedule does not settle on delivery"
    );
    assert_eq!(tally(&handle).await?.ticks, 5);

    ticks.cancel();
    runtime.shutdown_all().await
}

/// Skip and coalesce, asserted as an exact count under both cadences. Three
/// intervals of clock in one jump must produce **one** message, not three: a
/// schedule that fell behind must never flood the actor it was pacing.
#[acton_test]
async fn three_intervals_at_once_deliver_exactly_one_tick() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;

    for cadence in [Cadence::FixedRate, Cadence::FixedDelay] {
        let clock = Arc::new(ManualClock::new());
        let handle = start_tally(&mut runtime).await;

        let ticks = handle
            .with_clock(clock.clone())
            .send_every(Tick, step(), cadence);

        clock.advance(STEP * 3);

        assert_eq!(delivered(&ticks, 1).await, 1);
        assert_eq!(
            ticks.deliveries(),
            1,
            "{cadence:?} must coalesce three missed intervals into one tick"
        );
        assert_eq!(
            tally(&handle).await?.ticks,
            1,
            "{cadence:?} must deliver one message to the actor, not three"
        );

        ticks.cancel();
    }

    runtime.shutdown_all().await
}

/// Where the two cadences part company. A tick delivered two and a half
/// intervals late puts fixed-rate back on its original grid and pushes
/// fixed-delay a full interval past where it landed — so the same half-interval
/// advance delivers a second tick to one and nothing to the other.
#[acton_test]
async fn a_late_tick_separates_fixed_rate_from_fixed_delay() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());

    let paced = start_tally(&mut runtime).await;
    let spaced = start_tally(&mut runtime).await;

    // Both armed at the same instant: a `ManualClock` has not moved in between.
    let fixed_rate = paced
        .with_clock(clock.clone())
        .send_every(Tick, step(), Cadence::FixedRate);
    let fixed_delay =
        spaced
            .with_clock(clock.clone())
            .send_every(Tick, step(), Cadence::FixedDelay);

    clock.advance(STEP * 2 + part_of_step(1, 2));
    assert_eq!(delivered(&fixed_rate, 1).await, 1);
    assert_eq!(delivered(&fixed_delay, 1).await, 1);

    // Fixed-rate's next deadline is the third grid point, half an interval
    // away. Fixed-delay's is a whole interval from where it just fired.
    clock.advance(part_of_step(1, 2));

    assert_eq!(delivered(&fixed_rate, 2).await, 2);
    assert_eq!(
        fixed_delay.deliveries(),
        1,
        "fixed-delay measures from the last send, so it is not due yet"
    );

    assert_eq!(tally(&paced).await?.ticks, 2);
    assert_eq!(tally(&spaced).await?.ticks, 1);

    fixed_rate.cancel();
    fixed_delay.cancel();
    runtime.shutdown_all().await
}

/// Cancelling a repeating schedule stops it for good, not just until the next
/// deadline.
#[acton_test]
async fn cancelling_a_repeating_schedule_stops_it_for_good() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let ticks = handle
        .with_clock(clock.clone())
        .send_every(Tick, step(), Cadence::FixedRate);

    clock.advance(STEP);
    assert_eq!(delivered(&ticks, 1).await, 1);

    ticks.cancel();
    assert_eq!(settled(&ticks).await, ScheduledSendOutcome::Cancelled);

    clock.advance(STEP * 100);

    assert_eq!(
        ticks.deliveries(),
        1,
        "no tick may follow a settled cancellation"
    );
    assert_eq!(tally(&handle).await?.ticks, 1);

    runtime.shutdown_all().await
}

/// The deadline a repeating schedule is currently waiting on moves forward with
/// each tick. Without this the two cadences would be indistinguishable from
/// outside.
#[acton_test]
async fn the_pending_deadline_moves_forward_with_each_tick() -> anyhow::Result<()> {
    let mut runtime = ActonApp::launch_async().await;
    let clock = Arc::new(ManualClock::new());
    let handle = start_tally(&mut runtime).await;

    let armed_at = clock.now();
    let ticks = handle
        .with_clock(clock.clone())
        .send_every(Tick, step(), Cadence::FixedRate);

    assert_eq!(
        ticks.due_at().map(FireAt::instant),
        Some(armed_at + STEP),
        "the first deadline is one interval out"
    );

    clock.advance(STEP);
    assert_eq!(delivered(&ticks, 1).await, 1);

    assert_eq!(
        ticks.due_at().map(FireAt::instant),
        Some(armed_at + STEP * 2),
        "the next deadline is the next grid point"
    );

    ticks.cancel();
    assert_eq!(settled(&ticks).await, ScheduledSendOutcome::Cancelled);
    assert_eq!(
        ticks.due_at(),
        None,
        "a settled schedule is waiting on nothing"
    );

    runtime.shutdown_all().await
}
