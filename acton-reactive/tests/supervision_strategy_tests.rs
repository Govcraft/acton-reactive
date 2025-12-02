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

//! Integration tests for supervision strategies.

use acton_reactive::prelude::*;

/// Tests that the default supervision strategy is `OneForOne`.
#[tokio::test]
async fn test_default_supervision_strategy() {
    assert_eq!(
        SupervisionStrategy::default(),
        SupervisionStrategy::OneForOne
    );
}

/// Tests supervision strategy decision logic for `OneForOne`.
#[tokio::test]
async fn test_one_for_one_restarts_single_child() {
    let notification = ChildTerminated::new(
        Ern::with_root("child").unwrap(),
        TerminationReason::Panic("test".into()),
        RestartPolicy::Permanent,
    );

    let decision = SupervisionStrategy::OneForOne.decide(&notification, 0);
    assert_eq!(decision, SupervisionDecision::RestartChild);
}

/// Tests supervision strategy decision logic for `OneForAll`.
#[tokio::test]
async fn test_one_for_all_restarts_all_children() {
    let notification = ChildTerminated::new(
        Ern::with_root("child").unwrap(),
        TerminationReason::Panic("test".into()),
        RestartPolicy::Permanent,
    );

    let decision = SupervisionStrategy::OneForAll.decide(&notification, 0);
    assert_eq!(decision, SupervisionDecision::RestartAll);
}

/// Tests supervision strategy decision logic for `RestForOne`.
#[tokio::test]
async fn test_rest_for_one_restarts_from_index() {
    let notification = ChildTerminated::new(
        Ern::with_root("child").unwrap(),
        TerminationReason::Panic("test".into()),
        RestartPolicy::Permanent,
    );

    let decision = SupervisionStrategy::RestForOne.decide(&notification, 2);
    assert_eq!(decision, SupervisionDecision::RestartFrom(2));
}

/// Tests that temporary policy prevents restart regardless of strategy.
#[tokio::test]
async fn test_temporary_policy_prevents_restart_for_all_strategies() {
    let notification = ChildTerminated::new(
        Ern::with_root("child").unwrap(),
        TerminationReason::Panic("test".into()),
        RestartPolicy::Temporary,
    );

    for strategy in [
        SupervisionStrategy::OneForOne,
        SupervisionStrategy::OneForAll,
        SupervisionStrategy::RestForOne,
    ] {
        let decision = strategy.decide(&notification, 0);
        assert_eq!(
            decision,
            SupervisionDecision::NoRestart,
            "Strategy {strategy:?} should not restart with Temporary policy"
        );
    }
}

/// Tests that transient policy respects normal termination.
#[tokio::test]
async fn test_transient_policy_no_restart_on_normal() {
    let notification = ChildTerminated::new(
        Ern::with_root("child").unwrap(),
        TerminationReason::Normal,
        RestartPolicy::Transient,
    );

    let decision = SupervisionStrategy::OneForOne.decide(&notification, 0);
    assert_eq!(decision, SupervisionDecision::NoRestart);
}

/// Tests that transient policy restarts on panic.
#[tokio::test]
async fn test_transient_policy_restarts_on_panic() {
    let notification = ChildTerminated::new(
        Ern::with_root("child").unwrap(),
        TerminationReason::Panic("test".into()),
        RestartPolicy::Transient,
    );

    let decision = SupervisionStrategy::OneForOne.decide(&notification, 0);
    assert_eq!(decision, SupervisionDecision::RestartChild);
}

/// Tests that parent shutdown never triggers restart.
#[tokio::test]
async fn test_parent_shutdown_never_restarts() {
    let notification = ChildTerminated::new(
        Ern::with_root("child").unwrap(),
        TerminationReason::ParentShutdown,
        RestartPolicy::Permanent,
    );

    for strategy in [
        SupervisionStrategy::OneForOne,
        SupervisionStrategy::OneForAll,
        SupervisionStrategy::RestForOne,
    ] {
        let decision = strategy.decide(&notification, 0);
        assert_eq!(
            decision,
            SupervisionDecision::NoRestart,
            "Strategy {strategy:?} should not restart on ParentShutdown"
        );
    }
}

/// Tests `requires_group_restart` for different strategies.
#[tokio::test]
async fn test_requires_group_restart() {
    assert!(
        !SupervisionStrategy::OneForOne.requires_group_restart(),
        "OneForOne should not require group restart"
    );
    assert!(
        SupervisionStrategy::OneForAll.requires_group_restart(),
        "OneForAll should require group restart"
    );
    assert!(
        SupervisionStrategy::RestForOne.requires_group_restart(),
        "RestForOne should require group restart"
    );
}

/// Tests that `ActorConfig` can be configured with a supervision strategy.
#[tokio::test]
async fn test_actor_config_with_supervision_strategy() -> anyhow::Result<()> {
    // Test that the builder method works (we can't check the value since it's pub(crate),
    // but we verify the builder pattern compiles and runs without error)
    let _config = ActorConfig::new(Ern::with_root("supervisor")?, None, None)?
        .with_supervision_strategy(SupervisionStrategy::OneForAll);

    Ok(())
}
