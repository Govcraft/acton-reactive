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

use std::fmt;

use serde::{Deserialize, Serialize};

/// What a supervisor does when a child exhausts its restart allowance.
///
/// Restarting is only worth attempting a bounded number of times: a child that
/// fails immediately on every start will fail again no matter how many times it
/// is recreated. This policy decides what happens once the supervisor stops
/// trying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Escalation {
    /// Log the failure, notify this actor's own parent if it has one, leave the
    /// child stopped, and keep the supervisor running.
    ///
    /// The default, because in this framework a supervisor is usually also a
    /// working actor with responsibilities beyond its children. Stopping it
    /// because one child could not be kept alive would take down unrelated work.
    #[default]
    NotifyParent,

    /// Stop the supervisor itself, cascading to its remaining children.
    ///
    /// The Erlang/OTP behaviour: a supervisor that exceeds its restart
    /// intensity terminates and lets its own supervisor deal with it. Choose
    /// this when the supervisor's children are interdependent, so that one of
    /// them being permanently unavailable makes the rest meaningless.
    StopSupervisor,
}

impl fmt::Display for Escalation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::NotifyParent => "notify_parent",
            Self::StopSupervisor => "stop_supervisor",
        };
        f.write_str(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keeps_the_supervisor_running() {
        assert_eq!(Escalation::default(), Escalation::NotifyParent);
    }

    #[test]
    fn displays_in_snake_case_like_the_other_policy_enums() {
        assert_eq!(Escalation::NotifyParent.to_string(), "notify_parent");
        assert_eq!(Escalation::StopSupervisor.to_string(), "stop_supervisor");
    }

    #[test]
    fn round_trips_through_toml_as_a_config_value() {
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
        struct Config {
            escalation: Escalation,
        }

        for escalation in [Escalation::NotifyParent, Escalation::StopSupervisor] {
            let config = Config { escalation };
            let encoded = toml::to_string(&config).expect("Config serializes to TOML");
            let decoded: Config = toml::from_str(&encoded).expect("Config deserializes from TOML");
            assert_eq!(decoded, config);
        }
    }
}
