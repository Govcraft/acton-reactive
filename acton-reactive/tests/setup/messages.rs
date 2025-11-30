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
#[acton_message]
pub struct Pong;

/// Simple message often used to trigger an action or request in tests.
#[acton_message]
pub struct Ping;

/// Represents different types of jokes a `Comedian` actor might tell.
#[acton_message]
pub enum FunnyJoke {
    ChickenCrossesRoad,
    Pun,
}

/// Represents a joke targeted at a specific child actor or identified by a string.
#[acton_message]
pub enum FunnyJokeFor {
    ChickenCrossesRoad(Ern),
    Pun(String),
}

/// Represents audience reactions to a joke.
#[acton_message]
pub enum AudienceReactionMsg {
    Chuckle,
    Groan,
}

/// Generic message representing a joke being told (content not specified).
#[acton_message]
pub struct Joke;

/// Message used to instruct the `Counter` actor to increment its count.
#[acton_message]
pub struct Tally;

/// Message used by an actor to report its status, often including a count.
#[acton_message]
pub enum StatusReport {
    Complete(usize),
}

/// Message used to increment a counter.
#[acton_message]
pub struct Increment;
