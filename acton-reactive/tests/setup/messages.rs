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
#![allow(unused)] // Allow unused messages as this is a common setup file for various tests

use acton_reactive::prelude::*;

/// Simple message often used for replies or acknowledgements in tests.
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub struct Pong;

/// Simple message often used to trigger an action or request in tests.
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub struct Ping;

/// Represents different types of jokes a `Comedian` agent might tell.
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub enum FunnyJoke {
    ChickenCrossesRoad,
    Pun,
}

/// Represents a joke targeted at a specific child agent or identified by a string.
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub enum FunnyJokeFor {
    ChickenCrossesRoad(Ern),
    Pun(String),
}

/// Represents audience reactions to a joke.
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub enum AudienceReactionMsg {
    Chuckle,
    Groan,
}

/// Generic message representing a joke being told (content not specified).
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub struct Joke;

/// Message used to instruct the `Counter` agent to increment its count.
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub struct Tally;

/// Message used by an agent to report its status, often including a count.
/// Note: Relies on the blanket `impl<T> ActonMessage for T` in acton-core.
#[derive(Clone, Debug)]
pub enum StatusReport {
    Complete(usize),
}

#[acton_message]
pub struct Increment;
