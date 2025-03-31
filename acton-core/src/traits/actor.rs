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

use std::future::Future;

use acton_ern::{Ern};
use async_trait::async_trait;
use dashmap::DashMap;
use tokio_util::task::TaskTracker;
use tracing::*;

use crate::common::*;
use crate::message::{BrokerRequest, MessageAddress};
use crate::traits::acton_message::ActonMessage;

/// Defines the standard operations available on an [`AgentHandle`].
///
/// This trait provides methods for interacting with an agent, such as sending messages,
/// managing its lifecycle (stopping), accessing its identity (`id`, `name`), and
/// interacting with its children in a hierarchy.
#[async_trait]
pub trait AgentHandleInterface {
    /// Returns the message address for this agent.
    fn reply_address(&self) -> MessageAddress;
    /// Returns an envelope for the specified recipient and message, ready to send.
    fn create_envelope(&self, recipient_address: Option<MessageAddress>) -> OutboundEnvelope;
    /// Returns a map of the actor's children.
    fn children(&self) -> DashMap<String, AgentHandle>;

    /// Finds a child actor by its ERN.
    ///
    /// # Arguments
    ///
    /// * `arn` - The ERN of the child actor to find.
    ///
    /// # Returns
    ///
    /// An `Option<ActorRef>` containing the child actor if found, or `None` if not found.
    fn find_child(&self, id: &Ern) -> Option<AgentHandle>;

    /// Returns the actor's task tracker.
    fn tracker(&self) -> TaskTracker;

    /// Returns the actor's ERN.
    fn id(&self) -> Ern;

    /// Returns the actor's root from the ERN.
    fn name(&self) -> String;

    /// Creates a clone of the actor's reference.
    fn clone_ref(&self) -> AgentHandle;

    /// Emits a message from the actor, possibly to a pool item.
    ///
    /// # Arguments
    ///
    /// * `message` - The message to emit, implementing `ActonMessage`.
    ///
    /// # Returns
    ///
    /// A `Future` that resolves when the message has been emitted.
    #[instrument(skip(self), fields(children = self.children().len()))]
    fn send(
        &self,
        message: impl ActonMessage,
    ) -> impl Future<Output=()> + Send + Sync + '_
    where
        Self: Sync,
    {
        async move {
            let envelope = self.create_envelope(None);
            trace!("Envelope sender is {:?}", envelope.return_address.sender.root.to_string());
            envelope.send(message).await;
        }
    }
    /// Send a message synchronously.
    fn send_sync(&self, message: impl ActonMessage, recipient: &AgentHandle) -> anyhow::Result<()>
    where
        Self: AgentHandleInterface,
    {
        let envelope = self.create_envelope(Some(recipient.reply_address()));
        envelope.reply(BrokerRequest::new(message))?;
        Ok(())
    }

    /// Suspends the actor.
    fn stop(&self) -> impl Future<Output=anyhow::Result<()>> + Send + Sync + '_;
}
