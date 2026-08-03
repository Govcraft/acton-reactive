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

//! Every `From:` marker in `tests/docs_examples/` must name a page that exists.
//!
//! Those tests read as proof that the published documentation compiles and runs.
//! That guarantee is only worth anything if the page each one cites is real, and
//! nothing else checks it: a page can be renamed or deleted and the marker keeps
//! pointing at nothing, silently. Five of thirteen cited pages had gone missing
//! before this test existed.
//!
//! This is the reverse of the usual documentation risk. The docs did not drift
//! from the tested code; the test drifted from a page that moved.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Where the documentation site lives, relative to this crate's manifest.
///
/// Absent in a packaged crate, which is why a missing directory skips rather
/// than fails: the check is meaningful in the workspace and vacuous outside it.
const DOCS_SITE: &str = "../acton-docs-site/src/app";

fn workspace_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Extracts the page path from a `From: docs/.../page.md` marker, if the line has one.
///
/// Pure so the parsing is testable on its own, without a filesystem.
fn cited_page(line: &str) -> Option<&str> {
    let rest = line.split_once("From: ")?.1;
    let page = rest
        .split_once(" - ")
        .map_or(rest, |(page, _section)| page)
        .trim();
    page.strip_suffix("page.md").map(|_| page)
}

/// Every page cited by any test in `tests/docs_examples/`, deduplicated.
fn cited_pages(dir: &Path) -> std::io::Result<BTreeSet<String>> {
    let mut pages = BTreeSet::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let contents = std::fs::read_to_string(&path)?;
        for line in contents.lines() {
            if let Some(page) = cited_page(line) {
                pages.insert(page.to_string());
            }
        }
    }
    Ok(pages)
}

#[test]
fn every_cited_documentation_page_exists() {
    let docs_site = workspace_path(DOCS_SITE);
    if !docs_site.is_dir() {
        // Packaged crate: the site is not shipped, so there is nothing to check.
        return;
    }

    let examples = workspace_path("tests/docs_examples");
    let pages = cited_pages(&examples).expect("tests/docs_examples must be readable");

    assert!(
        !pages.is_empty(),
        "no `From:` markers found; this test would pass vacuously"
    );

    let missing: Vec<&String> = pages
        .iter()
        .filter(|page| !docs_site.join(page).is_file())
        .collect();

    assert!(
        missing.is_empty(),
        "these documentation pages are cited by tests in tests/docs_examples/ but do \
         not exist under {}:\n{}\n\nEither restore the page, or retarget the `From:` \
         marker at the page that now covers the material.",
        DOCS_SITE,
        missing
            .iter()
            .map(|page| format!("  - {page}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn a_marker_with_a_section_still_names_its_page() {
    assert_eq!(
        cited_page(r#"/// From: docs/pub-sub/page.md - "The Broker""#),
        Some("docs/pub-sub/page.md"),
    );
}

#[test]
fn a_marker_without_a_section_names_its_page() {
    assert_eq!(
        cited_page("//! Tests for examples from docs/handler-types/page.md"),
        None,
        "only lines using the `From:` marker are provenance claims",
    );
    assert_eq!(
        cited_page("/// From: docs/handler-types/page.md"),
        Some("docs/handler-types/page.md"),
    );
}

#[test]
fn a_line_with_no_marker_is_not_a_citation() {
    assert_eq!(cited_page("/// Ordinary documentation."), None);
}
