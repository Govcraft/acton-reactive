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
#![forbid(unsafe_code)]

//! Acton Macro Library
//!
//! This library provides procedural macros for the Acton actor framework.
//! It includes macros to derive common traits and boilerplate code for Acton messages
//! and actors.
//!
//! # Message Macro
//!
//! The [`acton_message`] macro simplifies creating message types for actor communication:
//!
//! ```ignore
//! // Basic message for internal actor communication
//! #[acton_message]
//! pub struct Ping;
//!
//! // IPC-enabled message with serialization support
//! #[acton_message(ipc)]
//! pub struct Request {
//!     pub query: String,
//! }
//! ```
//!
//! # Actor Macro
//!
//! The [`acton_actor`] macro simplifies creating actor state types:
//!
//! ```ignore
//! #[acton_actor]
//! pub struct Counter {
//!     count: i32,
//! }
//! ```

use proc_macro::TokenStream;

use quote::quote;
use syn::{parse_macro_input, DeriveInput};

fn has_derive(input: &DeriveInput, trait_name: &str) -> bool {
    input.attrs.iter().any(|attr| {
        if attr.path().is_ident("derive") {
            let mut found = false;
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident(trait_name) {
                    found = true;
                }
                Ok(())
            });
            found
        } else {
            false
        }
    })
}


/// Configuration options parsed from `#[acton_message(...)]` attributes.
#[derive(Default)]
struct MessageConfig {
    /// Enable serde serialization for IPC support.
    ipc: bool,
}

impl MessageConfig {
    /// Parse configuration from attribute tokens.
    fn parse(attr: &TokenStream) -> Self {
        let mut config = Self::default();

        // Parse the attribute stream to look for known options
        let attr_string = attr.to_string();
        for part in attr_string.split(',') {
            let trimmed = part.trim();
            if trimmed == "ipc" {
                config.ipc = true;
            }
        }

        config
    }
}

/// A procedural macro to derive the necessary traits for an Acton message.
///
/// This macro automatically implements the traits required for a type to be used
/// as a message in the Acton actor framework. It ensures compile-time verification
/// that the message type satisfies `Send + Sync` bounds.
///
/// # Basic Usage
///
/// ```ignore
/// use acton_macro::acton_message;
///
/// #[acton_message]
/// pub struct Ping;
///
/// #[acton_message]
/// pub struct Increment {
///     pub amount: u32,
/// }
/// ```
///
/// This expands to:
/// - `#[derive(Clone, Debug)]` (if not already present)
/// - A compile-time assertion that the type is `Send + Sync + 'static`
///
/// # IPC Support
///
/// For messages that need to cross process boundaries via IPC, use the `ipc` option:
///
/// ```ignore
/// use acton_macro::acton_message;
///
/// #[acton_message(ipc)]
/// pub struct Request {
///     pub query: String,
/// }
/// ```
///
/// This additionally derives `serde::Serialize` and `serde::Deserialize`.
///
/// **Note:** The `ipc` option requires `serde` to be available in scope. When using
/// `acton-reactive` with the `ipc` feature enabled, serde is automatically available.
#[proc_macro_attribute]
pub fn acton_message(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse configuration from attributes
    let config = MessageConfig::parse(&attr);

    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(item as DeriveInput);

    // Get the name and generics of the struct.
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Determine which traits need to be derived
    let need_clone = !has_derive(&input, "Clone");
    let need_debug = !has_derive(&input, "Debug");

    // Build the list of traits to derive
    let derives = {
        let mut traits = Vec::new();
        if need_clone {
            traits.push(quote!(Clone));
        }
        if need_debug {
            traits.push(quote!(Debug));
        }
        if config.ipc {
            // Only add serde derives if not already present
            if !has_derive(&input, "Serialize") {
                traits.push(quote!(serde::Serialize));
            }
            if !has_derive(&input, "Deserialize") {
                traits.push(quote!(serde::Deserialize));
            }
        }
        if traits.is_empty() {
            quote!()
        } else {
            quote!(#[derive(#(#traits),*)])
        }
    };

    // Generate a unique identifier for the static assertion to avoid conflicts
    let assert_ident = quote::format_ident!("_AssertActonMessage_{}", name);

    let expanded = quote! {
        #derives
        #input

        // Compile-time assertion that the message type satisfies Send + Sync + 'static.
        // This catches invalid message types early with clear error messages.
        #[doc(hidden)]
        #[allow(dead_code, non_camel_case_types, non_snake_case, clippy::needless_lifetimes)]
        const _: () = {
            fn #assert_ident #impl_generics () #where_clause {
                fn assert_bounds<T: Send + Sync + 'static>() {}
                assert_bounds::<#name #ty_generics>();
            }
        };
    };

    // Return the generated tokens.
    TokenStream::from(expanded)
}

/// A procedural macro to derive boilerplate traits for Acton actors.
///
/// This macro automatically implements the traits required for a type to be used
/// as an actor's state (model) in the Acton framework. It provides compile-time
/// verification that the actor type satisfies necessary bounds.
///
/// # Usage
///
/// ```ignore
/// use acton_macro::acton_actor;
///
/// #[acton_actor]
/// pub struct Counter {
///     count: i32,
/// }
///
/// #[acton_actor]
/// pub struct ChatRoom {
///     messages: Vec<String>,
///     participants: Vec<String>,
/// }
/// ```
///
/// This expands to:
/// - `#[derive(Default, Clone, Debug)]` (only traits not already present)
/// - A compile-time assertion that the type is `Send + 'static`
///
/// # Note
///
/// Actor state types must implement `Default` because actors are initialized
/// with their default state before handlers are registered.
#[proc_macro_attribute]
pub fn acton_actor(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(item as DeriveInput);

    // Get the name and generics of the struct.
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Determine which traits need to be derived
    let need_default = !has_derive(&input, "Default");
    let need_clone = !has_derive(&input, "Clone");
    let need_debug = !has_derive(&input, "Debug");

    // Build the list of traits to derive
    let derives = {
        let mut traits = Vec::new();
        if need_default {
            traits.push(quote!(Default));
        }
        if need_clone {
            traits.push(quote!(Clone));
        }
        if need_debug {
            traits.push(quote!(Debug));
        }
        if traits.is_empty() {
            quote!()
        } else {
            quote!(#[derive(#(#traits),*)])
        }
    };

    // Generate a unique identifier for the static assertion to avoid conflicts
    let assert_ident = quote::format_ident!("_AssertActonActor_{}", name);

    let expanded = quote! {
        #derives
        #input

        // Compile-time assertion that the actor type satisfies Send + 'static.
        // This catches invalid actor types early with clear error messages.
        #[doc(hidden)]
        #[allow(dead_code, non_camel_case_types, non_snake_case, clippy::needless_lifetimes)]
        const _: () = {
            fn #assert_ident #impl_generics () #where_clause {
                fn assert_bounds<T: Send + 'static>() {}
                assert_bounds::<#name #ty_generics>();
            }
        };
    };

    // Return the generated tokens.
    TokenStream::from(expanded)
}
