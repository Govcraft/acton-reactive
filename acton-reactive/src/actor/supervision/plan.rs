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

//! Deciding what to do about a terminated child.
//!
//! This module is the supervision subsystem's decision layer, kept separate
//! from the code that carries the decisions out. Nothing here spawns, stops, or
//! messages an actor; [`evaluate`] and [`plan_restart`] read their inputs and
//! return a description of what should happen. The engine performs it.
//!
//! Splitting it this way means the restart rules — which are where the subtle
//! bugs live — are tested by calling a function and comparing a value, with no
//! runtime, no actors, no timing, and no sleeping.
//!
//! Time is a parameter rather than something read from the clock, so a test can
//! place events days apart without waiting.

use std::time::{Duration, Instant};

use super::{BackoffDelay, ChildIndex, SupervisionDecision, SupervisionStrategy};
use crate::actor::{RestartLimitExceeded, RestartLimiter};
use crate::message::ChildTerminated;

/// Which children to stop and which to (re)start, in execution order.
///
/// The two lists are filtered differently and are not simply reverses of each
/// other: a child that is already down needs no stopping but may still need
/// starting, and a child the supervisor cannot recreate may still need
/// stopping.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestartPlan {
    /// Children to stop, in reverse start order — later children stop first.
    ///
    /// Contains only children that are currently running.
    pub stop: Vec<ChildIndex>,

    /// Children to start, in start order — earlier children come back first.
    ///
    /// Contains only children the supervisor is able to recreate.
    pub restart: Vec<ChildIndex>,
}

impl RestartPlan {
    /// Returns `true` when the plan asks for nothing at all.
    ///
    /// A supervisor uses this to avoid arming a backoff timer that would fire
    /// only to do no work.
    pub const fn is_empty(&self) -> bool {
        self.stop.is_empty() && self.restart.is_empty()
    }
}

/// Read-only snapshot of one child slot, copied out of the registry so that
/// planning stays pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotView {
    /// The child's position in the supervisor's start-ordered child list.
    pub index: ChildIndex,

    /// Whether the supervisor holds a blueprint and can therefore recreate it.
    pub restartable: bool,

    /// Whether the child is currently running.
    pub alive: bool,
}

/// Everything the planner needs to know about the child that just terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSnapshot {
    /// The failed child's position in the supervisor's child list.
    pub index: ChildIndex,

    /// Whether the supervisor holds a blueprint and can therefore recreate it.
    pub restartable: bool,

    /// Whether the supervisor asked for this stop itself.
    ///
    /// Set while a group restart is stopping siblings, so their terminations
    /// are recognised as expected rather than treated as fresh failures.
    pub expected_stop: bool,

    /// When this child was last restarted, if it ever has been.
    ///
    /// Used to decide whether the backoff should keep compounding or start
    /// over: a child that has been up longer than the limiter's window has
    /// demonstrably recovered.
    pub last_restart: Option<Instant>,
}

/// What a supervisor should do about one terminated child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisionOutcome {
    /// The supervisor asked for this stop; take no further action.
    Ignore,

    /// Let the child stay down.
    ///
    /// Either the supervisor cannot recreate it, or its restart policy says
    /// this termination does not warrant a restart.
    Forget,

    /// Carry out this plan once the backoff has elapsed.
    Restart {
        /// Which children to stop and start.
        plan: RestartPlan,
        /// How long to wait first.
        backoff: BackoffDelay,
    },

    /// The child exhausted its restart allowance; restarting it is no longer
    /// worth attempting.
    Escalate(RestartLimitExceeded),
}

/// Translates a strategy decision into a concrete stop/restart plan.
///
/// `stop` lists running children in reverse start order; `restart` lists
/// recreatable children in start order.
///
/// The two filters differ, which produces one genuinely surprising rule: a
/// sibling the supervisor cannot recreate is still **stopped** by a group
/// restart, and simply never comes back. That is deliberate. The point of
/// [`SupervisionStrategy::OneForAll`] is that the children are interdependent,
/// so leaving one running against a freshly restarted set would expose exactly
/// the inconsistent state the strategy exists to prevent.
///
/// Out-of-range indices are filtered rather than indexed, so a stale or
/// malformed index yields an empty plan instead of a panic.
pub fn plan_restart(
    decision: &SupervisionDecision,
    failed: ChildIndex,
    slots: &[SlotView],
) -> RestartPlan {
    let from = match decision {
        // Only the failed child, which is already down and so needs no stop.
        SupervisionDecision::RestartChild => {
            let restart = slots
                .iter()
                .filter(|slot| slot.index == failed && slot.restartable)
                .map(|slot| slot.index)
                .collect();
            return RestartPlan {
                stop: Vec::new(),
                restart,
            };
        }
        SupervisionDecision::RestartAll => ChildIndex::new(0),
        SupervisionDecision::RestartFrom(index) => ChildIndex::new(*index),
        SupervisionDecision::NoRestart | SupervisionDecision::Escalate => {
            return RestartPlan::default()
        }
    };

    let mut affected: Vec<&SlotView> = slots.iter().filter(|slot| slot.index >= from).collect();
    affected.sort_unstable_by_key(|slot| slot.index);

    let restart = affected
        .iter()
        .filter(|slot| slot.restartable)
        .map(|slot| slot.index)
        .collect();

    let stop = affected
        .iter()
        .rev()
        .filter(|slot| slot.alive)
        .map(|slot| slot.index)
        .collect();

    RestartPlan { stop, restart }
}

/// Decides what a supervisor should do about a terminated child.
///
/// `now` is a parameter rather than a clock read so the decision is
/// deterministic and testable. `limiter` is borrowed mutably because recording
/// a restart and computing its backoff are one operation.
///
/// The order of the checks is the correctness core of the whole subsystem:
///
/// 1. An expected stop is ignored. This has to come first. Stopping a sibling
///    during a group restart produces [`TerminationReason::Normal`], and a
///    [`Permanent`] child restarts on a normal termination, so any other
///    ordering makes a group restart loop forever.
/// 2. A child with no blueprint is forgotten, before the limiter is touched.
///    This is what keeps existing programs behaving exactly as they did:
///    children registered through the legacy `supervise()` path have no
///    blueprint, so nothing about them changes.
/// 3. Only then is the strategy consulted, and only then is a restart charged
///    against the limiter.
///
/// [`TerminationReason::Normal`]: crate::actor::TerminationReason::Normal
/// [`Permanent`]: crate::actor::RestartPolicy::Permanent
pub fn evaluate(
    notification: &ChildTerminated,
    slot: &SlotSnapshot,
    strategy: SupervisionStrategy,
    limiter: &mut RestartLimiter,
    limiter_window: Duration,
    slots: &[SlotView],
    now: Instant,
) -> SupervisionOutcome {
    // 1. The supervisor asked for this stop; it is not a failure.
    if slot.expected_stop {
        return SupervisionOutcome::Ignore;
    }

    // 2. No blueprint means the supervisor cannot bring this child back. Return
    //    before consulting the limiter so legacy children never consume an
    //    allowance they can never use.
    if !slot.restartable {
        return SupervisionOutcome::Forget;
    }

    // 3. Reuse the existing policy table rather than reimplementing it. This is
    //    where ParentShutdown and the Temporary policy drop out for free.
    let decision = strategy.decide(notification, slot.index.get());
    if matches!(decision, SupervisionDecision::NoRestart) {
        return SupervisionOutcome::Forget;
    }

    // 4. A child that stayed up longer than the limiter's window has recovered,
    //    so the backoff starts over instead of compounding from its last crash.
    if slot
        .last_restart
        .is_some_and(|last| now.saturating_duration_since(last) > limiter_window)
    {
        limiter.reset_consecutive();
    }

    // 5. Out of restarts: escalate rather than keep trying.
    if let Err(exceeded) = limiter.can_restart() {
        return SupervisionOutcome::Escalate(exceeded);
    }

    // 6. Charge the restart and take its backoff in one call.
    let backoff = BackoffDelay::from(limiter.record_restart());
    let plan = plan_restart(&decision, slot.index, slots);

    if plan.is_empty() {
        SupervisionOutcome::Forget
    } else {
        SupervisionOutcome::Restart { plan, backoff }
    }
}

#[cfg(test)]
mod tests {
    use acton_ern::Ern;

    use super::*;
    use crate::actor::{RestartLimiterConfig, RestartPolicy, TerminationReason};

    fn view(index: usize, restartable: bool, alive: bool) -> SlotView {
        SlotView {
            index: ChildIndex::new(index),
            restartable,
            alive,
        }
    }

    /// Four children, all recreatable and all running.
    fn four_healthy_slots() -> Vec<SlotView> {
        (0..4).map(|index| view(index, true, true)).collect()
    }

    fn indices(raw: &[usize]) -> Vec<ChildIndex> {
        raw.iter().copied().map(ChildIndex::new).collect()
    }

    fn notification(policy: RestartPolicy, reason: TerminationReason) -> ChildTerminated {
        ChildTerminated::new(
            Ern::with_root("child").expect("'child' is a valid Ern root"),
            reason,
            policy,
        )
    }

    fn snapshot(index: usize) -> SlotSnapshot {
        SlotSnapshot {
            index: ChildIndex::new(index),
            restartable: true,
            expected_stop: false,
            last_restart: None,
        }
    }

    /// The limiter window, defined once so the limiter's own `window_secs` and
    /// the `limiter_window` argument passed to `evaluate` cannot drift apart.
    const WINDOW_SECS: u64 = 60;

    fn window() -> Duration {
        Duration::from_secs(WINDOW_SECS)
    }

    fn limiter(max_restarts: u32, initial_backoff_ms: u64, max_backoff_ms: u64) -> RestartLimiter {
        RestartLimiter::new(RestartLimiterConfig {
            enabled: true,
            max_restarts,
            window_secs: WINDOW_SECS,
            initial_backoff_ms,
            max_backoff_ms,
            backoff_multiplier: 2.0,
        })
    }

    // ---- plan_restart ----------------------------------------------------

    #[test]
    fn restart_child_restarts_only_the_failed_child_at_any_position() {
        let slots = four_healthy_slots();

        for position in 0..4 {
            let plan = plan_restart(
                &SupervisionDecision::RestartChild,
                ChildIndex::new(position),
                &slots,
            );
            assert_eq!(plan.restart, indices(&[position]));
            assert!(
                plan.stop.is_empty(),
                "the failed child is already down, so nothing needs stopping"
            );
        }
    }

    #[test]
    fn restart_child_plans_nothing_when_the_child_cannot_be_recreated() {
        let slots = vec![view(0, true, true), view(1, false, true)];

        let plan = plan_restart(
            &SupervisionDecision::RestartChild,
            ChildIndex::new(1),
            &slots,
        );

        assert_eq!(plan, RestartPlan::default());
    }

    #[test]
    fn restart_all_stops_in_reverse_order_and_restarts_in_start_order() {
        let slots = four_healthy_slots();

        let plan = plan_restart(&SupervisionDecision::RestartAll, ChildIndex::new(0), &slots);

        assert_eq!(plan.restart, indices(&[0, 1, 2, 3]));
        assert_eq!(plan.stop, indices(&[3, 2, 1, 0]));
    }

    #[test]
    fn restart_all_skips_stopping_a_child_that_is_already_down() {
        let mut slots = four_healthy_slots();
        slots[2].alive = false;

        let plan = plan_restart(&SupervisionDecision::RestartAll, ChildIndex::new(2), &slots);

        assert_eq!(plan.stop, indices(&[3, 1, 0]), "slot 2 is already down");
        assert_eq!(
            plan.restart,
            indices(&[0, 1, 2, 3]),
            "slot 2 still needs starting"
        );
    }

    #[test]
    fn restart_all_stops_a_sibling_it_cannot_recreate() {
        // The surprising rule, pinned deliberately: a legacy child with no
        // blueprint is still brought down for consistency, and never returns.
        let mut slots = four_healthy_slots();
        slots[1].restartable = false;

        let plan = plan_restart(&SupervisionDecision::RestartAll, ChildIndex::new(0), &slots);

        assert!(
            plan.stop.contains(&ChildIndex::new(1)),
            "an interdependent sibling must come down: {:?}",
            plan.stop
        );
        assert!(
            !plan.restart.contains(&ChildIndex::new(1)),
            "it cannot be recreated: {:?}",
            plan.restart
        );
    }

    #[test]
    fn rest_for_one_touches_only_the_failed_child_and_those_after_it() {
        // The failed child has already terminated, so it needs no stopping —
        // only the siblings started after it do.
        let mut slots = four_healthy_slots();
        slots[2].alive = false;

        let plan = plan_restart(
            &SupervisionDecision::RestartFrom(2),
            ChildIndex::new(2),
            &slots,
        );

        assert_eq!(plan.stop, indices(&[3]), "slot 2 is already down");
        assert_eq!(plan.restart, indices(&[2, 3]));
        for untouched in [ChildIndex::new(0), ChildIndex::new(1)] {
            assert!(!plan.stop.contains(&untouched));
            assert!(!plan.restart.contains(&untouched));
        }
    }

    #[test]
    fn rest_for_one_from_the_first_child_matches_restart_all() {
        let slots = four_healthy_slots();

        let rest_for_one = plan_restart(
            &SupervisionDecision::RestartFrom(0),
            ChildIndex::new(0),
            &slots,
        );
        let one_for_all =
            plan_restart(&SupervisionDecision::RestartAll, ChildIndex::new(0), &slots);

        assert_eq!(rest_for_one, one_for_all);
    }

    #[test]
    fn rest_for_one_uses_the_strategys_index_over_the_failed_index() {
        let slots = four_healthy_slots();

        // The decision carries the authoritative position; `failed` disagrees.
        let plan = plan_restart(
            &SupervisionDecision::RestartFrom(3),
            ChildIndex::new(0),
            &slots,
        );

        assert_eq!(plan.restart, indices(&[3]));
    }

    #[test]
    fn no_restart_and_escalate_plan_nothing() {
        let slots = four_healthy_slots();

        for decision in [
            SupervisionDecision::NoRestart,
            SupervisionDecision::Escalate,
        ] {
            let plan = plan_restart(&decision, ChildIndex::new(1), &slots);
            assert_eq!(plan, RestartPlan::default(), "{decision}");
            assert!(plan.is_empty());
        }
    }

    #[test]
    fn an_out_of_range_index_plans_nothing_instead_of_panicking() {
        let slots = four_healthy_slots();

        let by_failed = plan_restart(
            &SupervisionDecision::RestartChild,
            ChildIndex::new(9),
            &slots,
        );
        assert_eq!(by_failed, RestartPlan::default());

        let by_decision = plan_restart(
            &SupervisionDecision::RestartFrom(9),
            ChildIndex::new(9),
            &slots,
        );
        assert_eq!(by_decision, RestartPlan::default());
    }

    #[test]
    fn an_empty_child_list_plans_nothing_instead_of_panicking() {
        for decision in [
            SupervisionDecision::RestartChild,
            SupervisionDecision::RestartAll,
            SupervisionDecision::RestartFrom(0),
            SupervisionDecision::NoRestart,
            SupervisionDecision::Escalate,
        ] {
            let plan = plan_restart(&decision, ChildIndex::new(0), &[]);
            assert_eq!(plan, RestartPlan::default(), "{decision}");
        }
    }

    #[test]
    fn stop_always_descends_and_restart_always_ascends() {
        // Property over a slot list supplied out of start order, with a mix of
        // dead and non-recreatable children.
        let slots = vec![
            view(3, true, true),
            view(0, true, false),
            view(2, false, true),
            view(1, true, true),
        ];

        let plan = plan_restart(&SupervisionDecision::RestartAll, ChildIndex::new(0), &slots);

        assert!(
            plan.stop.windows(2).all(|pair| pair[0] > pair[1]),
            "stop must strictly descend: {:?}",
            plan.stop
        );
        assert!(
            plan.restart.windows(2).all(|pair| pair[0] < pair[1]),
            "restart must strictly ascend: {:?}",
            plan.restart
        );
    }

    // ---- evaluate --------------------------------------------------------

    #[test]
    fn an_expected_stop_is_ignored_even_though_the_policy_would_restart_it() {
        // The anti-loop guard. Stopping a sibling for a group restart reports
        // Normal, and Permanent restarts on Normal, so checking the policy
        // first would restart the child we just deliberately stopped.
        let mut limiter = limiter(5, 100, 10_000);
        let slot = SlotSnapshot {
            expected_stop: true,
            ..snapshot(0)
        };

        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Normal),
            &slot,
            SupervisionStrategy::OneForAll,
            &mut limiter,
            window(),
            &four_healthy_slots(),
            Instant::now(),
        );

        assert_eq!(outcome, SupervisionOutcome::Ignore);
        assert_eq!(
            limiter.restarts_in_window(),
            0,
            "an ignored stop must not consume a restart"
        );
    }

    #[test]
    fn a_child_without_a_blueprint_is_forgotten_without_touching_the_limiter() {
        // The backward-compatibility guarantee for the legacy supervise() path.
        let mut limiter = limiter(5, 100, 10_000);
        let slot = SlotSnapshot {
            restartable: false,
            ..snapshot(0)
        };

        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
            &slot,
            SupervisionStrategy::OneForOne,
            &mut limiter,
            window(),
            &four_healthy_slots(),
            Instant::now(),
        );

        assert_eq!(outcome, SupervisionOutcome::Forget);
        assert_eq!(limiter.restarts_in_window(), 0);
        assert_eq!(limiter.consecutive_restarts(), 0);
    }

    #[test]
    fn a_temporary_child_is_never_restarted() {
        for reason in [
            TerminationReason::Normal,
            TerminationReason::Panic("x".into()),
            TerminationReason::InboxClosed,
            TerminationReason::ParentShutdown,
        ] {
            let mut limiter = limiter(5, 100, 10_000);
            let outcome = evaluate(
                &notification(RestartPolicy::Temporary, reason.clone()),
                &snapshot(0),
                SupervisionStrategy::OneForOne,
                &mut limiter,
                window(),
                &four_healthy_slots(),
                Instant::now(),
            );

            assert_eq!(outcome, SupervisionOutcome::Forget, "{reason}");
        }
    }

    #[test]
    fn a_transient_child_restarts_only_on_abnormal_termination() {
        let mut normal = limiter(5, 100, 10_000);
        let after_normal = evaluate(
            &notification(RestartPolicy::Transient, TerminationReason::Normal),
            &snapshot(0),
            SupervisionStrategy::OneForOne,
            &mut normal,
            window(),
            &four_healthy_slots(),
            Instant::now(),
        );
        assert_eq!(after_normal, SupervisionOutcome::Forget);

        let mut panicked = limiter(5, 100, 10_000);
        let after_panic = evaluate(
            &notification(RestartPolicy::Transient, TerminationReason::Panic("x".into())),
            &snapshot(0),
            SupervisionStrategy::OneForOne,
            &mut panicked,
            window(),
            &four_healthy_slots(),
            Instant::now(),
        );
        assert!(matches!(after_panic, SupervisionOutcome::Restart { .. }));
    }

    #[test]
    fn a_parent_shutdown_never_restarts_even_a_permanent_child() {
        let mut limiter = limiter(5, 100, 10_000);

        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::ParentShutdown),
            &snapshot(0),
            SupervisionStrategy::OneForOne,
            &mut limiter,
            window(),
            &four_healthy_slots(),
            Instant::now(),
        );

        assert_eq!(outcome, SupervisionOutcome::Forget);
    }

    #[test]
    fn backoff_compounds_across_consecutive_restarts_and_then_caps() {
        let mut limiter = limiter(10, 100, 300);
        let now = Instant::now();
        let slots = four_healthy_slots();

        let mut delays = Vec::new();
        for _ in 0..4 {
            let outcome = evaluate(
                &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
                &snapshot(0),
                SupervisionStrategy::OneForOne,
                &mut limiter,
                window(),
                &slots,
                now,
            );
            match outcome {
                SupervisionOutcome::Restart { backoff, .. } => delays.push(backoff),
                other => panic!("expected a restart, got {other:?}"),
            }
        }

        assert_eq!(delays[0], BackoffDelay::from(Duration::from_millis(100)));
        assert_eq!(delays[1], BackoffDelay::from(Duration::from_millis(200)));
        assert_eq!(delays[2], BackoffDelay::from(Duration::from_millis(300)));
        assert_eq!(
            delays[3],
            BackoffDelay::from(Duration::from_millis(300)),
            "capped at max_backoff_ms"
        );
    }

    #[test]
    fn a_child_that_stayed_up_past_the_window_starts_its_backoff_over() {
        let window = window();
        let mut limiter = limiter(10, 100, 10_000);
        let start = Instant::now();
        let slots = four_healthy_slots();

        // Two crashes in quick succession compound the backoff to 200ms.
        for _ in 0..2 {
            let _ = evaluate(
                &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
                &snapshot(0),
                SupervisionStrategy::OneForOne,
                &mut limiter,
                window,
                &slots,
                start,
            );
        }
        assert_eq!(limiter.consecutive_restarts(), 2);

        // A third crash, long after the child last came back, starts over.
        let recovered = SlotSnapshot {
            last_restart: Some(start),
            ..snapshot(0)
        };
        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
            &recovered,
            SupervisionStrategy::OneForOne,
            &mut limiter,
            window,
            &slots,
            start + window + Duration::from_secs(1),
        );

        match outcome {
            SupervisionOutcome::Restart { backoff, .. } => assert_eq!(
                backoff,
                BackoffDelay::from(Duration::from_millis(100)),
                "the backoff should start over, not compound to 400ms"
            ),
            other => panic!("expected a restart, got {other:?}"),
        }
    }

    #[test]
    fn a_crash_inside_the_window_keeps_compounding_the_backoff() {
        let window = window();
        let mut limiter = limiter(10, 100, 10_000);
        let start = Instant::now();
        let slots = four_healthy_slots();

        let _ = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
            &snapshot(0),
            SupervisionStrategy::OneForOne,
            &mut limiter,
            window,
            &slots,
            start,
        );

        let recently_restarted = SlotSnapshot {
            last_restart: Some(start),
            ..snapshot(0)
        };
        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
            &recently_restarted,
            SupervisionStrategy::OneForOne,
            &mut limiter,
            window,
            &slots,
            start + Duration::from_secs(5),
        );

        match outcome {
            SupervisionOutcome::Restart { backoff, .. } => {
                assert_eq!(backoff, BackoffDelay::from(Duration::from_millis(200)));
            }
            other => panic!("expected a restart, got {other:?}"),
        }
    }

    #[test]
    fn exhausting_the_allowance_escalates_instead_of_restarting() {
        let mut limiter = limiter(2, 100, 10_000);
        let now = Instant::now();
        let slots = four_healthy_slots();

        for attempt in 0..2 {
            let outcome = evaluate(
                &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
                &snapshot(0),
                SupervisionStrategy::OneForOne,
                &mut limiter,
                window(),
                &slots,
                now,
            );
            assert!(
                matches!(outcome, SupervisionOutcome::Restart { .. }),
                "attempt {attempt} should still restart"
            );
        }

        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
            &snapshot(0),
            SupervisionStrategy::OneForOne,
            &mut limiter,
            window(),
            &slots,
            now,
        );

        match outcome {
            SupervisionOutcome::Escalate(exceeded) => {
                assert_eq!(exceeded.attempts, 2);
                assert_eq!(exceeded.max_restarts, 2);
                assert_eq!(exceeded.window_secs, WINDOW_SECS);
            }
            other => panic!("expected an escalation, got {other:?}"),
        }
    }

    #[test]
    fn a_disabled_limiter_never_escalates_and_never_delays() {
        let mut limiter = RestartLimiter::new(RestartLimiterConfig::disabled());
        let now = Instant::now();
        let slots = four_healthy_slots();

        for attempt in 0..20 {
            let outcome = evaluate(
                &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
                &snapshot(0),
                SupervisionStrategy::OneForOne,
                &mut limiter,
                window(),
                &slots,
                now,
            );

            match outcome {
                SupervisionOutcome::Restart { backoff, .. } => {
                    assert_eq!(backoff, BackoffDelay::NONE, "attempt {attempt}");
                    assert!(backoff.is_immediate());
                }
                other => panic!("expected a restart on attempt {attempt}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_restart_that_would_plan_nothing_is_forgotten_rather_than_scheduled() {
        // The child is recreatable and the policy allows a restart, but it is
        // absent from the slot list, so there is nothing to schedule.
        let mut limiter = limiter(5, 100, 10_000);

        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
            &snapshot(0),
            SupervisionStrategy::OneForOne,
            &mut limiter,
            window(),
            &[],
            Instant::now(),
        );

        assert_eq!(outcome, SupervisionOutcome::Forget);
    }

    #[test]
    fn a_group_strategy_produces_a_group_plan() {
        let mut limiter = limiter(5, 100, 10_000);

        let outcome = evaluate(
            &notification(RestartPolicy::Permanent, TerminationReason::Panic("x".into())),
            &snapshot(1),
            SupervisionStrategy::RestForOne,
            &mut limiter,
            window(),
            &four_healthy_slots(),
            Instant::now(),
        );

        match outcome {
            SupervisionOutcome::Restart { plan, .. } => {
                assert_eq!(plan.restart, indices(&[1, 2, 3]));
                assert_eq!(plan.stop, indices(&[3, 2, 1]));
            }
            other => panic!("expected a restart, got {other:?}"),
        }
    }
}
