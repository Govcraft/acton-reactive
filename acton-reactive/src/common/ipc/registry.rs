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

//! IPC type registry for message deserialization.

use std::sync::Arc;

use dashmap::DashMap;
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::types::IpcError;
use crate::traits::ActonMessage;

/// Type alias for the deserializer function stored in the registry.
///
/// The function takes raw bytes and returns either a boxed `ActonMessage`
/// trait object or a serialization error string.
type DeserializerFn = Arc<
    dyn Fn(&[u8]) -> Result<Box<dyn ActonMessage + Send + Sync>, String> + Send + Sync,
>;

/// Type alias for the serializer function stored in the registry.
///
/// The function takes a trait object reference and returns JSON bytes.
type SerializerFn = Arc<
    dyn Fn(&dyn ActonMessage) -> Result<serde_json::Value, String> + Send + Sync,
>;

/// Registry mapping message type names to deserializers.
///
/// The `IpcTypeRegistry` is the central component for IPC message deserialization.
/// Before external messages can be routed to agents, their types must be registered
/// here so the runtime knows how to convert serialized bytes back into typed messages.
///
/// # Thread Safety
///
/// The registry uses [`DashMap`] internally and is safe to access concurrently
/// from multiple threads. Registration and deserialization can happen simultaneously.
///
/// # Example
///
/// ```rust,ignore
/// use acton_reactive::prelude::*;
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Clone, Debug, Serialize, Deserialize)]
/// struct PriceUpdate {
///     symbol: String,
///     price: f64,
/// }
///
/// let registry = IpcTypeRegistry::new();
///
/// // Register with a custom name (recommended for stability)
/// registry.register::<PriceUpdate>("PriceUpdate");
///
/// // Or use the Rust type name (less stable across versions)
/// registry.register_with_type_name::<PriceUpdate>();
/// ```
#[derive(Default)]
pub struct IpcTypeRegistry {
    /// Maps type names to deserializer functions.
    deserializers: DashMap<String, DeserializerFn>,
    /// Maps `TypeId` to type names for reverse lookup during broadcast forwarding.
    type_id_to_name: DashMap<std::any::TypeId, String>,
    /// Maps `TypeId` to serializer functions for broadcast forwarding.
    serializers: DashMap<std::any::TypeId, SerializerFn>,
}

impl std::fmt::Debug for IpcTypeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpcTypeRegistry")
            .field("registered_types", &self.deserializers.len())
            .finish()
    }
}

impl IpcTypeRegistry {
    /// Creates a new, empty type registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            deserializers: DashMap::new(),
            type_id_to_name: DashMap::new(),
            serializers: DashMap::new(),
        }
    }

    /// Registers a message type with a custom name.
    ///
    /// The name is used as the key for looking up the deserializer when
    /// processing incoming IPC messages. Using stable, semantic names
    /// (like `"PriceUpdate"`) is recommended over Rust type paths which
    /// may change between versions.
    ///
    /// # Type Parameters
    ///
    /// * `M`: The message type. Must implement [`ActonMessage`], [`Serialize`],
    ///   and [`DeserializeOwned`].
    ///
    /// # Arguments
    ///
    /// * `name`: The type name to register. This must match the `message_type`
    ///   field in incoming [`IpcEnvelope`](super::IpcEnvelope) messages.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// registry.register::<PriceUpdate>("PriceUpdate");
    /// ```
    pub fn register<M>(&self, name: &str)
    where
        M: ActonMessage + Serialize + DeserializeOwned + 'static,
    {
        let deserializer: DeserializerFn = Arc::new(|bytes: &[u8]| {
            let msg: M = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            Ok(Box::new(msg))
        });
        self.deserializers.insert(name.to_string(), deserializer);

        let type_id = std::any::TypeId::of::<M>();

        // Store the reverse mapping for broadcast forwarding
        self.type_id_to_name.insert(type_id, name.to_string());

        // Store serializer for broadcast forwarding
        let serializer: SerializerFn = Arc::new(|msg: &dyn ActonMessage| {
            // Downcast to the concrete type and serialize
            let concrete = msg
                .as_any()
                .downcast_ref::<M>()
                .ok_or_else(|| "Type mismatch during serialization".to_string())?;
            serde_json::to_value(concrete).map_err(|e| e.to_string())
        });
        self.serializers.insert(type_id, serializer);
    }

    /// Registers a message type using its Rust type name.
    ///
    /// Uses [`std::any::type_name`] to derive the registration name.
    /// This is convenient but less stable than using a custom name,
    /// as type paths can change with code reorganization.
    ///
    /// # Type Parameters
    ///
    /// * `M`: The message type. Must implement [`ActonMessage`], [`Serialize`],
    ///   and [`DeserializeOwned`].
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// registry.register_with_type_name::<my_crate::PriceUpdate>();
    /// // Registers as "my_crate::PriceUpdate"
    /// ```
    pub fn register_with_type_name<M>(&self)
    where
        M: ActonMessage + Serialize + DeserializeOwned + 'static,
    {
        let type_name = std::any::type_name::<M>();
        self.register::<M>(type_name);
    }

    /// Deserializes bytes into a message using the registered deserializer.
    ///
    /// Looks up the deserializer by type name and applies it to the provided bytes.
    ///
    /// # Arguments
    ///
    /// * `type_name`: The message type name (must match a registered type).
    /// * `bytes`: The serialized message bytes (JSON format).
    ///
    /// # Returns
    ///
    /// A boxed [`ActonMessage`] trait object on success, or an [`IpcError`] on failure.
    ///
    /// # Errors
    ///
    /// * [`IpcError::UnknownMessageType`] - If no deserializer is registered for the type.
    /// * [`IpcError::SerializationError`] - If deserialization fails.
    pub fn deserialize(
        &self,
        type_name: &str,
        bytes: &[u8],
    ) -> Result<Box<dyn ActonMessage + Send + Sync>, IpcError> {
        let deserializer = self
            .deserializers
            .get(type_name)
            .ok_or_else(|| IpcError::UnknownMessageType(type_name.to_string()))?;

        deserializer(bytes).map_err(IpcError::SerializationError)
    }

    /// Deserializes a JSON value into a message.
    ///
    /// Convenience method that serializes the JSON value to bytes first,
    /// then deserializes using the registered deserializer.
    ///
    /// # Arguments
    ///
    /// * `type_name`: The message type name (must match a registered type).
    /// * `value`: The JSON value to deserialize.
    ///
    /// # Returns
    ///
    /// A boxed [`ActonMessage`] trait object on success, or an [`IpcError`] on failure.
    pub fn deserialize_value(
        &self,
        type_name: &str,
        value: &serde_json::Value,
    ) -> Result<Box<dyn ActonMessage + Send + Sync>, IpcError> {
        let bytes = serde_json::to_vec(value)?;
        self.deserialize(type_name, &bytes)
    }

    /// Checks if a message type is registered.
    ///
    /// # Arguments
    ///
    /// * `type_name`: The type name to check.
    ///
    /// # Returns
    ///
    /// `true` if the type is registered, `false` otherwise.
    #[must_use]
    pub fn is_registered(&self, type_name: &str) -> bool {
        self.deserializers.contains_key(type_name)
    }

    /// Returns the number of registered message types.
    #[must_use]
    pub fn len(&self) -> usize {
        self.deserializers.len()
    }

    /// Returns `true` if no message types are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.deserializers.is_empty()
    }

    /// Returns an iterator over registered type names.
    pub fn type_names(&self) -> impl Iterator<Item = String> + '_ {
        self.deserializers.iter().map(|entry| entry.key().clone())
    }

    /// Gets the type name for a given `TypeId`.
    ///
    /// This is used during broadcast forwarding to look up the IPC type name
    /// for a message's `TypeId`.
    ///
    /// # Arguments
    ///
    /// * `type_id`: The `TypeId` to look up.
    ///
    /// # Returns
    ///
    /// The registered type name if found, or `None` if the type is not registered.
    #[must_use]
    pub fn get_type_name_by_id(&self, type_id: &std::any::TypeId) -> Option<String> {
        self.type_id_to_name.get(type_id).map(|r| r.clone())
    }

    /// Serializes a message to JSON using the registered serializer.
    ///
    /// This is used during broadcast forwarding to serialize messages for IPC clients.
    ///
    /// # Arguments
    ///
    /// * `type_id`: The `TypeId` of the message.
    /// * `message`: The message to serialize (as a trait object).
    ///
    /// # Returns
    ///
    /// A JSON value on success, or an error string on failure.
    pub fn serialize_by_type_id(
        &self,
        type_id: &std::any::TypeId,
        message: &dyn ActonMessage,
    ) -> Result<serde_json::Value, String> {
        let serializer = self
            .serializers
            .get(type_id)
            .ok_or_else(|| "Type not registered for IPC serialization".to_string())?;
        serializer(message)
    }
}

// Manual Clone implementation since DeserializerFn and SerializerFn are Arc-wrapped
impl Clone for IpcTypeRegistry {
    fn clone(&self) -> Self {
        let new_deserializers = DashMap::new();
        for entry in &self.deserializers {
            new_deserializers.insert(entry.key().clone(), entry.value().clone());
        }
        let new_type_id_map = DashMap::new();
        for entry in &self.type_id_to_name {
            new_type_id_map.insert(*entry.key(), entry.value().clone());
        }
        let new_serializers = DashMap::new();
        for entry in &self.serializers {
            new_serializers.insert(*entry.key(), entry.value().clone());
        }
        Self {
            deserializers: new_deserializers,
            type_id_to_name: new_type_id_map,
            serializers: new_serializers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
    struct TestMessage {
        value: i32,
        text: String,
    }

    #[test]
    fn test_register_and_deserialize() {
        let registry = IpcTypeRegistry::new();
        registry.register::<TestMessage>("TestMessage");

        assert!(registry.is_registered("TestMessage"));
        assert!(!registry.is_registered("Unknown"));

        let msg = TestMessage {
            value: 42,
            text: "hello".to_string(),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();

        let result = registry.deserialize("TestMessage", &bytes);
        assert!(result.is_ok());

        let boxed = result.unwrap();
        // Dereference the box before calling as_any() to get the inner type's TypeId
        let downcast = (*boxed).as_any().downcast_ref::<TestMessage>();
        assert!(downcast.is_some());
        assert_eq!(downcast.unwrap(), &msg);
    }

    #[test]
    fn test_register_with_type_name() {
        let registry = IpcTypeRegistry::new();
        registry.register_with_type_name::<TestMessage>();

        let type_name = std::any::type_name::<TestMessage>();
        assert!(registry.is_registered(type_name));
    }

    #[test]
    fn test_deserialize_unknown_type() {
        let registry = IpcTypeRegistry::new();
        let result = registry.deserialize("Unknown", b"{}");

        assert!(matches!(result, Err(IpcError::UnknownMessageType(_))));
    }

    #[test]
    fn test_deserialize_invalid_json() {
        let registry = IpcTypeRegistry::new();
        registry.register::<TestMessage>("TestMessage");

        let result = registry.deserialize("TestMessage", b"not valid json");
        assert!(matches!(result, Err(IpcError::SerializationError(_))));
    }

    #[test]
    fn test_deserialize_value() {
        let registry = IpcTypeRegistry::new();
        registry.register::<TestMessage>("TestMessage");

        let value = serde_json::json!({
            "value": 42,
            "text": "hello"
        });

        let result = registry.deserialize_value("TestMessage", &value);
        assert!(result.is_ok());
    }

    #[test]
    fn test_registry_len_and_empty() {
        let registry = IpcTypeRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);

        registry.register::<TestMessage>("TestMessage");
        assert!(!registry.is_empty());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn test_type_names_iterator() {
        let registry = IpcTypeRegistry::new();
        registry.register::<TestMessage>("Type1");
        registry.register::<TestMessage>("Type2");

        let names: Vec<String> = registry.type_names().collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"Type1".to_string()));
        assert!(names.contains(&"Type2".to_string()));
    }

    #[test]
    fn test_registry_clone() {
        let registry = IpcTypeRegistry::new();
        registry.register::<TestMessage>("TestMessage");

        let cloned = registry.clone();
        assert!(cloned.is_registered("TestMessage"));

        // Verify both original and clone work independently
        let msg = TestMessage {
            value: 99,
            text: "cloned".to_string(),
        };
        let bytes = serde_json::to_vec(&msg).unwrap();

        // Original should still work
        let original_result = registry.deserialize("TestMessage", &bytes);
        assert!(original_result.is_ok());

        // Clone should also work
        let clone_result = cloned.deserialize("TestMessage", &bytes);
        assert!(clone_result.is_ok());

        // Verify the deserialized message from clone matches
        let boxed = clone_result.unwrap();
        let downcast = (*boxed).as_any().downcast_ref::<TestMessage>();
        assert!(downcast.is_some());
        assert_eq!(downcast.unwrap(), &msg);
    }
}
