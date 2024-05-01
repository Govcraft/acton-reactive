/*
 *
 *  * Copyright (c) 2024 Govcraft.
 *  *
 *  * Licensed under the Apache License, Version 2.0 (the "License");
 *  * you may not use this file except in compliance with the License.
 *  * You may obtain a copy of the License at
 *  *
 *  *     http://www.apache.org/licenses/LICENSE-2.0
 *  *
 *  * Unless required by applicable law or agreed to in writing, software
 *  * distributed under the License is distributed on an "AS IS" BASIS,
 *  * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 *  * See the License for the specific language governing permissions and
 *  * limitations under the License.
 *
 *
 */

use std::time::SystemTime;
use futures::SinkExt;
use tracing::{debug, instrument, trace};

use crate::common::{Envelope, MessageError, OutboundChannel};
use crate::prelude::QuasarMessage;


#[derive(Clone, Debug, Default)]
pub struct OutboundEnvelope {
    reply_to: Option<OutboundChannel>,
}

impl OutboundEnvelope {
    pub fn new(reply_to: Option<OutboundChannel>) -> Self {
        OutboundEnvelope { reply_to }
    }
    #[instrument(skip(self))]
    pub async fn reply(&self, message: impl QuasarMessage + Send + Sync + 'static) -> Result<(), MessageError> {
        trace!("{:?}", &message);
        let envelope = Envelope { message: Box::new(message), sent_time: SystemTime::now() };
        if let Some(reply_to) = &self.reply_to {
            reply_to.send(envelope).await?;
        }
        Ok(())
    }
}
