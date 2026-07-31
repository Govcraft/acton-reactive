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

//! The error type returned by the supervision subsystem.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use acton_ern::Ern;

use super::SupervisionState;
use crate::actor::RestartLimitExceeded;

/// Errors produced by the supervision subsystem.
///
/// This type is [`Clone`] because a supervisor publishes one outcome to a
/// caller that may only hold it by reference; returning it by value requires a
/// copy.
///
/// Because it implements [`Error`], it converts into `anyhow::Error` with `?`
/// at the existing call sites that return `anyhow::Result`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SupervisionError {
    /// This actor already supervises a child with this identifier.
    ///
    /// Reachable two ways, which need different remedies:
    ///
    /// - **A name collision.** Children built from a blueprint derive their
    ///   identifier from their parent and their name, so two of them sharing a
    ///   name under one parent are the same child. Rename one.
    /// - **A reused configuration.** Supervising twice with the same
    ///   [`ActorConfig`] reuses its identifier. Build a fresh one.
    ///
    /// [`ActorConfig::new`] and [`new_with_name`] mint a fresh identifier on
    /// every call, so children created through those cannot collide by name.
    ///
    /// [`ActorConfig`]: crate::actor::ActorConfig
    /// [`ActorConfig::new`]: crate::actor::ActorConfig::new
    /// [`new_with_name`]: crate::actor::ActorConfig::new_with_name
    DuplicateChild {
        /// The identifier that is already supervised.
        child: Ern,
    },

    /// No supervised child with this identifier exists.
    UnknownChild {
        /// The identifier that was looked up.
        child: Ern,
    },

    /// The child's configuration could not be used to create the actor.
    ConfigRejected {
        /// The child the configuration belongs to.
        child: Ern,
        /// Why the configuration was rejected.
        reason: String,
    },

    /// The supervising actor is no longer running.
    SupervisorStopped {
        /// The supervisor that was targeted.
        supervisor: Ern,
    },

    /// The child exhausted its restart allowance.
    RestartLimit {
        /// The child that exhausted its allowance.
        child: Ern,
        /// Details of the exceeded limit.
        limit: RestartLimitExceeded,
    },

    /// A child did not stop within the shutdown deadline.
    ChildStopTimeout {
        /// The child that failed to stop.
        child: Ern,
        /// How long the supervisor waited.
        waited: Duration,
    },

    /// The supervising actor's task ended before it released the child.
    ///
    /// The child is still supervised as far as anything can tell, and is still
    /// running unless the supervisor's own shutdown stopped it.
    ReleaseLost {
        /// The child whose release was never processed.
        child: Ern,
    },

    /// The child settled in a state it will not leave, without ever running.
    ///
    /// Returned to a caller waiting for a child to come up when the supervisor
    /// published a terminal state instead and recorded no more specific reason.
    ChildNotRunning {
        /// The child that was waited on.
        child: Ern,
        /// The state it settled in.
        state: SupervisionState,
    },

    /// The supervising actor's task ended before it recorded the registration.
    ///
    /// Distinct from [`SupervisionError::SupervisorStopped`]: the supervisor
    /// accepted the registration message but stopped before acting on it, so
    /// the child may have been started and left unsupervised.
    RegistrationLost {
        /// The child whose registration was never recorded.
        child: Ern,
    },
}

impl fmt::Display for SupervisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateChild { child } => write!(
                f,
                "child '{child}' is already supervised by this actor; give the child a different name, build a fresh ActorConfig, or call unsupervise() first to replace it"
            ),
            Self::UnknownChild { child } => write!(
                f,
                "no supervised child '{child}'; it may have been removed, or it was started with supervise() instead of supervise_with()"
            ),
            Self::ConfigRejected { child, reason } => {
                write!(f, "cannot create supervised child '{child}': {reason}")
            }
            Self::SupervisorStopped { supervisor } => write!(
                f,
                "supervisor '{supervisor}' has stopped; it can no longer supervise children"
            ),
            Self::RestartLimit { child, limit } => {
                write!(f, "child '{child}' exceeded its restart limit: {limit}")
            }
            Self::ChildStopTimeout { child, waited } => write!(
                f,
                "child '{child}' did not stop within {waited:?}; the supervisor gave up waiting"
            ),
            Self::ChildNotRunning { child, state } => write!(
                f,
                "child '{child}' is {state} and will not reach running; its supervisor recorded no reason"
            ),
            Self::RegistrationLost { child } => write!(
                f,
                "supervisor stopped before recording child '{child}'; the child may be running unsupervised and should be stopped"
            ),
            Self::ReleaseLost { child } => write!(
                f,
                "supervisor stopped before releasing child '{child}'; the child was not removed from supervision"
            ),
        }
    }
}

impl Error for SupervisionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::RestartLimit { limit, .. } => Some(limit),
            Self::DuplicateChild { .. }
            | Self::UnknownChild { .. }
            | Self::ConfigRejected { .. }
            | Self::SupervisorStopped { .. }
            | Self::ChildStopTimeout { .. }
            | Self::ChildNotRunning { .. }
            | Self::RegistrationLost { .. }
            | Self::ReleaseLost { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Ern::with_root` appends a generated, timestamp-based suffix, so two
    /// calls with the same name produce *different* identifiers. Every test
    /// builds one and clones it rather than calling this twice.
    fn child() -> Ern {
        Ern::with_root("worker").expect("'worker' is a valid Ern root")
    }

    fn exceeded() -> RestartLimitExceeded {
        RestartLimitExceeded {
            attempts: 5,
            max_restarts: 5,
            window_secs: 60,
        }
    }

    #[test]
    fn duplicate_child_message_names_the_remedy() {
        let child = child();
        let message = SupervisionError::DuplicateChild {
            child: child.clone(),
        }
        .to_string();
        assert!(message.contains(&child.to_string()), "{message}");
        assert!(message.contains("unsupervise()"), "{message}");
        // Both remedies must be present. Blueprint children derive their
        // identifier from parent plus name, so a rename genuinely resolves a
        // collision; reusing one ActorConfig is the other way in, and only a
        // fresh config resolves that.
        assert!(
            message.contains("different name"),
            "the collision case needs a rename: {message}"
        );
        assert!(
            message.contains("ActorConfig"),
            "the config-reuse case needs a fresh config: {message}"
        );
    }

    #[test]
    fn two_root_actors_built_from_the_same_name_never_share_an_identifier() {
        // Half of what DuplicateChild's wording rests on: `Ern::with_root`
        // carries a generated UUIDv7 suffix, so actors created through
        // `ActorConfig::new`/`new_with_name` cannot collide by name. The other
        // half — that blueprint children *can* — is pinned in actor_config.rs.
        let first = Ern::with_root("worker").expect("'worker' is a valid Ern root");
        let second = Ern::with_root("worker").expect("'worker' is a valid Ern root");

        assert_ne!(first, second);
        assert_ne!(first.to_string(), second.to_string());
    }

    #[test]
    fn unknown_child_message_explains_the_likely_cause() {
        let child = child();
        let message = SupervisionError::UnknownChild {
            child: child.clone(),
        }
        .to_string();
        assert!(message.contains(&child.to_string()), "{message}");
        assert!(message.contains("supervise_with()"), "{message}");
    }

    #[test]
    fn config_rejected_message_carries_the_reason() {
        let child = child();
        let message = SupervisionError::ConfigRejected {
            child: child.clone(),
            reason: "parent is not running".to_string(),
        }
        .to_string();
        assert!(message.contains(&child.to_string()), "{message}");
        assert!(message.contains("parent is not running"), "{message}");
    }

    #[test]
    fn supervisor_stopped_message_names_the_supervisor() {
        let supervisor = Ern::with_root("pool").expect("'pool' is a valid Ern root");
        let message = SupervisionError::SupervisorStopped {
            supervisor: supervisor.clone(),
        }
        .to_string();
        assert!(message.contains(&supervisor.to_string()), "{message}");
        assert!(message.contains("has stopped"), "{message}");
    }

    #[test]
    fn restart_limit_message_nests_the_limit_detail() {
        let child = child();
        let message = SupervisionError::RestartLimit {
            child: child.clone(),
            limit: exceeded(),
        }
        .to_string();
        assert!(message.contains(&child.to_string()), "{message}");
        // The nested Display of RestartLimitExceeded is included verbatim.
        assert!(message.contains("5 attempts"), "{message}");
        assert!(message.contains("60 seconds"), "{message}");
    }

    #[test]
    fn child_stop_timeout_message_reports_how_long_it_waited() {
        let child = child();
        let message = SupervisionError::ChildStopTimeout {
            child: child.clone(),
            waited: Duration::from_secs(5),
        }
        .to_string();
        assert!(message.contains(&child.to_string()), "{message}");
        assert!(message.contains("5s"), "{message}");
    }

    #[test]
    fn release_lost_message_says_the_child_is_still_supervised() {
        let child = child();
        let message = SupervisionError::ReleaseLost {
            child: child.clone(),
        }
        .to_string();
        assert!(message.contains(&child.to_string()), "{message}");
        assert!(message.contains("not removed"), "{message}");
    }

    #[test]
    fn registration_lost_message_warns_the_child_may_be_orphaned() {
        let child = child();
        let message = SupervisionError::RegistrationLost {
            child: child.clone(),
        }
        .to_string();
        assert!(message.contains(&child.to_string()), "{message}");
        assert!(message.contains("unsupervised"), "{message}");
    }

    #[test]
    fn only_restart_limit_exposes_a_source() {
        let child = child();
        let with_source = SupervisionError::RestartLimit {
            child: child.clone(),
            limit: exceeded(),
        };
        assert!(with_source.source().is_some());

        let without_source = [
            SupervisionError::DuplicateChild {
                child: child.clone(),
            },
            SupervisionError::UnknownChild {
                child: child.clone(),
            },
            SupervisionError::ConfigRejected {
                child: child.clone(),
                reason: "nope".to_string(),
            },
            SupervisionError::SupervisorStopped {
                supervisor: child.clone(),
            },
            SupervisionError::ChildStopTimeout {
                child: child.clone(),
                waited: Duration::from_secs(1),
            },
            SupervisionError::RegistrationLost {
                child: child.clone(),
            },
            SupervisionError::ReleaseLost { child },
        ];
        for error in &without_source {
            assert!(error.source().is_none(), "{error}");
        }
    }

    #[test]
    fn restart_limit_source_is_the_exceeded_limit() {
        let error = SupervisionError::RestartLimit {
            child: child(),
            limit: exceeded(),
        };
        let source = error.source().expect("RestartLimit exposes a source");
        assert_eq!(source.to_string(), exceeded().to_string());
    }

    #[test]
    fn converts_into_anyhow_error_via_question_mark() {
        fn fallible(child: Ern) -> anyhow::Result<()> {
            Err(SupervisionError::UnknownChild { child })?;
            Ok(())
        }

        let child = child();
        let error = fallible(child.clone()).expect_err("the function always fails");
        assert!(error.to_string().contains(&child.to_string()));
        assert!(error.downcast_ref::<SupervisionError>().is_some());
    }

    #[test]
    fn equal_variants_compare_equal() {
        let child = child();
        assert_eq!(
            SupervisionError::DuplicateChild {
                child: child.clone()
            },
            SupervisionError::DuplicateChild {
                child: child.clone()
            }
        );
        assert_ne!(
            SupervisionError::DuplicateChild {
                child: child.clone()
            },
            SupervisionError::UnknownChild { child }
        );
    }
}
