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

use crate::common::AgentSender;
use acton_ern::prelude::*;
use derive_new::new; // Keep using derive_new for the constructor
use tokio::sync::mpsc::channel;
use tokio_util::sync::CancellationToken;
/// Represents the addressable endpoint of an agent, combining its identity and inbox channel.
///
/// A `MessageAddress` contains the necessary information to send a message to a specific
/// agent: its unique identifier (`sender`, an [`Ern`]) and the sender half (`address`)
/// of the MPSC channel connected to its inbox.
///
/// This struct is typically used within message envelopes ([`OutboundEnvelope`]) to specify
/// the sender and recipient of a message.
#[derive(new, Clone, Debug)]
pub struct MessageAddress {
    /// The sender part of the MPSC channel for the agent's inbox.
    pub(crate) address: AgentSender,
    /// The unique identifier (`Ern`) of the agent associated with this address.
    pub(crate) sender: Ern,
    pub(crate) cancellation_token: CancellationToken,
}

impl MessageAddress {
    /// Returns the root name component of the agent's identifier (`Ern`).
    ///
    /// This provides a simple string representation of the agent's base name.
    #[inline]
    pub fn name(&self) -> &str {
        self.sender.root.as_str()
    }
}

impl Default for MessageAddress {
    /// Creates a default `MessageAddress` with a default `Ern` and a closed channel sender.
    ///
    /// This is primarily useful for placeholder initialization before a real address is known.
    /// Messages cannot be successfully sent using the default address's channel sender.
    fn default() -> Self {
        let (outbox, _) = tokio::sync::mpsc::channel(1); // Create a dummy, likely closed sender
        Self::new(outbox, Ern::default(), CancellationToken::default())
    }
}

// Unit tests remain unchanged and undocumented
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_return_address() {
        let return_address = MessageAddress::default();
        // The default sender might not *always* be closed immediately,
        // depending on MPSC channel implementation details, but it's not usable.
        // A better test might involve trying to send and expecting an error,
        // but that requires more setup. This assertion is okay for basic check.
        assert!(return_address.address.is_closed());
    }
}
