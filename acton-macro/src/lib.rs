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
//!
//! # Main Entry Point
//!
//! The [`acton_main`] macro provides a convenient entry point for Acton applications:
//!
//! ```ignore
//! use acton_reactive::prelude::*;
//!
//! #[acton_main]
//! async fn main() {
//!     let mut app = ActonApp::launch_async().await;
//!     // ... your application logic
//!     app.shutdown_all().await;
//! }
//! ```

use proc_macro::TokenStream;

use quote::quote;
use syn::{parse_macro_input, DeriveInput, ItemFn};

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

/// Configuration options parsed from `#[acton_actor(...)]` attributes.
#[derive(Default)]
struct ActorConfig {
    /// Skip deriving Default (user will implement it manually).
    no_default: bool,
}

impl ActorConfig {
    /// Parse configuration from attribute tokens.
    fn parse(attr: &TokenStream) -> Self {
        let mut config = Self::default();

        // Parse the attribute stream to look for known options
        let attr_string = attr.to_string();
        for part in attr_string.split(',') {
            let trimmed = part.trim();
            if trimmed == "no_default" {
                config.no_default = true;
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
/// - `#[derive(Default, Debug)]` (only traits not already present)
/// - A compile-time assertion that the type is `Send + 'static`
///
/// # Options
///
/// ## `no_default`
///
/// Skip deriving `Default` when you need to implement it manually (e.g., when
/// a field's type doesn't implement `Default`):
///
/// ```ignore
/// use std::io::{stdout, Stdout};
///
/// #[acton_actor(no_default)]
/// struct Printer {
///     out: Stdout,
/// }
///
/// impl Default for Printer {
///     fn default() -> Self {
///         Self { out: stdout() }
///     }
/// }
/// ```
///
/// # Note
///
/// Actor state types must implement `Default` because actors are initialized
/// with their default state before handlers are registered. When using
/// `no_default`, you must provide your own `Default` implementation.
#[proc_macro_attribute]
pub fn acton_actor(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse configuration from attributes
    let config = ActorConfig::parse(&attr);

    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(item as DeriveInput);

    // Get the name and generics of the struct.
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Determine which traits need to be derived
    let need_default = !config.no_default && !has_derive(&input, "Default");
    let need_debug = !has_derive(&input, "Debug");

    // Build the list of traits to derive
    let derives = {
        let mut traits = Vec::new();
        if need_default {
            traits.push(quote!(Default));
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

/// Entry point macro for Acton applications.
///
/// This macro marks an async function as the entry point for an Acton application,
/// setting up the async runtime automatically. It is a convenience wrapper that
/// eliminates the need to directly reference the underlying async runtime.
///
/// # Usage
///
/// ```ignore
/// use acton_reactive::prelude::*;
///
/// #[acton_main]
/// async fn main() {
///     let mut app = ActonApp::launch_async().await;
///     // ... your application logic
///     app.shutdown_all().await;
/// }
/// ```
///
/// # Configuration
///
/// The macro supports optional configuration for the runtime:
///
/// - `flavor`: The runtime flavor (`"multi_thread"` or `"current_thread"`)
/// - `worker_threads`: Number of worker threads (only for multi-threaded runtime)
///
/// ```ignore
/// // Use single-threaded runtime
/// #[acton_main(flavor = "current_thread")]
/// async fn main() { }
///
/// // Specify worker thread count
/// #[acton_main(worker_threads = 4)]
/// async fn main() { }
/// ```
///
/// The default is a multi-threaded runtime with the default number of worker threads.
#[proc_macro_attribute]
pub fn acton_main(attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemFn);

    let attrs = &input.attrs;
    let vis = &input.vis;
    let sig = &input.sig;
    let body = &input.block;

    // Validate that the function is async
    if sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            sig.fn_token,
            "the async keyword is missing from the function declaration",
        )
        .to_compile_error()
        .into();
    }

    // Validate function name is main
    if sig.ident != "main" {
        return syn::Error::new_spanned(
            &sig.ident, // Keep reference to avoid moving sig.ident which is used later
            "acton_main can only be applied to the main function",
        )
        .to_compile_error()
        .into();
    }

    // Parse configuration attributes
    let attr_string = attr.to_string();
    let use_current_thread = attr_string.contains("current_thread");

    // Extract worker_threads if specified
    let worker_threads: Option<usize> = attr_string
        .split(',')
        .find(|s| s.contains("worker_threads"))
        .and_then(|s| s.split('=').nth(1).and_then(|v| v.trim().parse().ok()));

    // Generate the runtime builder based on configuration
    let runtime_builder = if use_current_thread {
        quote! {
            ::acton_reactive::prelude::tokio::runtime::Builder::new_current_thread()
        }
    } else if let Some(threads) = worker_threads {
        quote! {
            ::acton_reactive::prelude::tokio::runtime::Builder::new_multi_thread()
                .worker_threads(#threads)
        }
    } else {
        quote! {
            ::acton_reactive::prelude::tokio::runtime::Builder::new_multi_thread()
        }
    };

    // Create the sync function signature (remove async)
    let fn_name = &sig.ident;
    let fn_inputs = &sig.inputs;
    let fn_output = &sig.output;

    let expanded = quote! {
        #(#attrs)*
        #vis fn #fn_name(#fn_inputs) #fn_output {
            #runtime_builder
                .enable_all()
                .build()
                .expect("Failed to build Acton runtime")
                .block_on(async #body)
        }
    };

    TokenStream::from(expanded)
}
