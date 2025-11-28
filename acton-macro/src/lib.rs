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
//! It includes macros to derive common traits and boilerplate code for Acton messages.

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


/// A procedural macro to derive the necessary traits for an Acton message.
///
/// This macro will automatically implement `Clone`, `Debug`, and `ActonMessage`
/// for the annotated type. The `Sync` trait is implemented automatically by the
/// compiler when possible, so an explicit unsafe implementation is unnecessary.
#[proc_macro_attribute]
pub fn acton_message(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(item as DeriveInput);

    // Get the name and generics of the struct.
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Generate the expanded code.
    let clone_attr = if has_derive(&input, "Clone") {
        quote!()
    } else {
        quote!(#[derive(Clone)])
    };

    let expanded = quote! {
        // Derive the Clone trait if not already present.
        #clone_attr
        #input

        // Implement the Debug trait.
        impl #impl_generics std::fmt::Debug for #name #ty_generics #where_clause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, stringify!(#name))
            }
        }

    };

    // Return the generated tokens.
    TokenStream::from(expanded)
}

/// A procedural macro to derive boilerplate traits for Acton actors.
///
/// This macro ensures the annotated type implements `Default`, `Clone`, and
/// `Debug`. Existing derives for these traits are preserved.
#[proc_macro_attribute]
pub fn acton_actor(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(item as DeriveInput);

    // Get the name and generics of the struct.
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Generate the expanded code.
    let need_default = !has_derive(&input, "Default");
    let need_clone = !has_derive(&input, "Clone");
    let derives = {
        let mut traits = Vec::new();
        if need_default { traits.push(quote!(Default)); }
        if need_clone { traits.push(quote!(Clone)); }
        if traits.is_empty() { quote!() } else { quote!(#[derive(#(#traits),*)]) }
    };

    let expanded = quote! {
        // Derive Default and Clone if not already present.
        #derives
        #input

        // Implement the Debug trait.
        impl #impl_generics std::fmt::Debug for #name #ty_generics #where_clause {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, stringify!(#name))
            }
        }
    };

    // Return the generated tokens.
    TokenStream::from(expanded)
}
