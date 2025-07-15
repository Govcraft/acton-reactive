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

use acton_ern::Ern;
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

use crate::common::{AgentHandle, BrokerRef, ActonConfig};

#[derive(Debug, Clone)]
pub(crate) struct ActonInner {
    pub(crate) broker: BrokerRef,
    pub(crate) roots: DashMap<Ern, AgentHandle>,
    pub(crate) cancellation_token: CancellationToken,
    pub(crate) config: ActonConfig,
}

impl Default for ActonInner {
    fn default() -> Self {
        Self {
            broker: Default::default(),
            roots: Default::default(),
            cancellation_token: CancellationToken::new(),
            config: ActonConfig::default(),
        }
    }
}
