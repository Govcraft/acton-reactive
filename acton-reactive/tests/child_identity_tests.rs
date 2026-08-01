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

//! What `ManagedActor::create_child` derives for a child's identifier.
//!
//! Until 9.0.0 this path built the child by parsing the parent's *display
//! string* back into an `Ern` and adding the child's own `Ern` to it. Two
//! defects composed there:
//!
//! * parsing re-stamped the root with a fresh `UUIDv7`, so the derivation was
//!   not deterministic and the result was not descended from the parent; and
//! * `Add for Ern` keeps the left root and concatenates parts, while
//!   `Ern::with_root(name)` puts the name in the *root* with no parts — so the
//!   child's name contributed nothing at all.
//!
//! Holding the parsed parent fixed, `alpha` and `beta` came out **identical**.
//! Siblings differed in practice only because each call drew a fresh random
//! suffix. That is why the tests below assert that each identifier *carries its
//! own name*: asserting only that two siblings differ passes on the broken
//! derivation, for the wrong reason.

use acton_reactive::prelude::*;

/// Bare state; these tests are about identity, not behaviour.
#[derive(Default, Debug)]
struct Service;

/// Two children named differently get identifiers that differ *because of their
/// names*.
#[tokio::test]
async fn two_children_with_different_names_carry_their_own_names() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor_with_name::<Service>("prices".to_string());

    let alpha = parent.create_child("alpha".to_string())?;
    let beta = parent.create_child("beta".to_string())?;

    let alpha_id = alpha.id().to_string();
    let beta_id = beta.id().to_string();

    // The load-bearing pair. `assert_ne!` alone would have passed before the
    // fix, when the name was discarded and the difference came from a fresh
    // random suffix on each call.
    assert!(
        alpha_id.ends_with("/alpha"),
        "the child's identifier should carry its own name: {alpha_id}"
    );
    assert!(
        beta_id.ends_with("/beta"),
        "the child's identifier should carry its own name: {beta_id}"
    );
    assert_ne!(alpha_id, beta_id);

    runtime.shutdown_all().await?;
    Ok(())
}

/// The same parent and the same name always yield the same identifier.
#[tokio::test]
async fn creating_the_same_child_twice_yields_the_same_identifier() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor_with_name::<Service>("prices".to_string());

    let first = parent.create_child("worker".to_string())?;
    let second = parent.create_child("worker".to_string())?;

    // Before the fix each call re-parsed the parent's display string, and
    // parsing mints a fresh `UUIDv7` root, so two calls never agreed.
    assert_eq!(
        first.id(),
        second.id(),
        "a child's identity must not drift between calls, or a supervisor \
         cannot rebuild a child under the identifier it registered"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// A child's identifier is its parent's, extended — not a fresh one.
#[tokio::test]
async fn a_child_identifier_reads_as_the_parent_then_the_name() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor_with_name::<Service>("prices".to_string());
    let parent_id = parent.id().to_string();

    let child = parent.create_child("alpha".to_string())?;
    let child_id = child.id().to_string();

    assert_eq!(
        child_id,
        format!("{parent_id}/alpha"),
        "the child should be descended from this parent, not from a re-minted one"
    );

    runtime.shutdown_all().await?;
    Ok(())
}

/// Grandchildren keep nesting, so the identifier records the whole ancestry.
#[tokio::test]
async fn a_grandchild_records_the_whole_ancestry() -> anyhow::Result<()> {
    let mut runtime: ActorRuntime = ActonApp::launch_async().await;
    let parent = runtime.new_actor_with_name::<Service>("prices".to_string());
    let parent_id = parent.id().to_string();

    let child = parent.create_child("alpha".to_string())?;
    let grandchild = child.create_child("deep".to_string())?;

    assert_eq!(
        grandchild.id().to_string(),
        format!("{parent_id}/alpha/deep")
    );

    runtime.shutdown_all().await?;
    Ok(())
}
