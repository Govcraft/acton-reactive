/*
 * Copyright (c) 2024. Govcraft
 *
 * Licensed under either of
 *   * Apache License, Version 2.0 (the "License");
 *     you may not use this file except in compliance with the License.
 *     you may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
 *   * MIT license: http://opensource.org/licenses/MIT
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the applicable License for the specific language governing permissions and
 * limitations under that License.
 */

//! Platform-specific discovery of Acton's configuration files.
//!
//! Every part of the crate that needs to know *where* configuration lives goes
//! through [`find_config_file`], so the platform difference is stated once:
//!
//! * **Unix** — the XDG base directory search path, under the `acton` prefix.
//!   `$XDG_CONFIG_HOME/acton/<relative>` first, then each entry of
//!   `$XDG_CONFIG_DIRS`.
//! * **Everything else** — `%APPDATA%\acton\<relative>`.

use std::fmt;
use std::path::{Path, PathBuf};

/// Directory name Acton's configuration lives under, on every platform.
const PREFIX: &str = "acton";

/// Why a configuration directory could not be located.
///
/// This is distinct from "no configuration file exists": that is reported as
/// `Ok(None)` by [`find_config_file`]. An error here means the search could not
/// be performed at all, which is worth logging more loudly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLocationError {
    /// The XDG base directory lookup failed (Unix targets).
    ///
    /// Carries the underlying cause rendered as text, because the `xdg` crate's
    /// error type is not in the dependency graph on non-Unix targets.
    #[cfg(any(unix, test))]
    BaseDirectories(String),

    /// `%APPDATA%` is unset or empty (non-Unix targets).
    #[cfg(any(not(unix), test))]
    AppDataUnset,
}

impl fmt::Display for ConfigLocationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(any(unix, test))]
            Self::BaseDirectories(cause) => write!(
                f,
                "could not determine the XDG configuration directories: {cause}; \
                 set HOME or XDG_CONFIG_HOME to a readable directory"
            ),
            #[cfg(any(not(unix), test))]
            Self::AppDataUnset => f.write_str(
                "the APPDATA environment variable is unset or empty, so there is no \
                 configuration directory to search; set APPDATA to the roaming \
                 application data directory",
            ),
        }
    }
}

impl std::error::Error for ConfigLocationError {}

/// Build the path a configuration file would occupy under an application data root.
///
/// Pure: it performs no I/O and consults no environment, so the layout rule can be
/// asserted on any platform.
#[cfg(any(not(unix), test))]
fn app_data_config_path(app_data_root: &Path, relative: &Path) -> PathBuf {
    app_data_root.join(PREFIX).join(relative)
}

/// Look for `relative` beneath an application data root, returning it only if it
/// is a readable file.
///
/// The root is a parameter rather than an environment read so that the rule is
/// exercised by tests on every platform, not only the one that ships it.
#[cfg(any(not(unix), test))]
fn find_in_app_data(app_data_root: &Path, relative: &Path) -> Option<PathBuf> {
    let candidate = app_data_config_path(app_data_root, relative);
    candidate.is_file().then_some(candidate)
}

/// Read the roaming application data root from `%APPDATA%`.
#[cfg(not(unix))]
fn app_data_root() -> Result<PathBuf, ConfigLocationError> {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(ConfigLocationError::AppDataUnset)
}

/// Locate a configuration file by its path relative to Acton's configuration
/// directory.
///
/// Returns `Ok(None)` when the search succeeded but no such file exists, and
/// `Err` when the platform's configuration directory could not be determined.
#[cfg(unix)]
pub fn find_config_file(relative: &Path) -> Result<Option<PathBuf>, ConfigLocationError> {
    let dirs = xdg::BaseDirectories::with_prefix(PREFIX)
        .map_err(|e| ConfigLocationError::BaseDirectories(e.to_string()))?;
    Ok(dirs.find_config_file(relative))
}

/// Locate a configuration file by its path relative to Acton's configuration
/// directory.
///
/// Returns `Ok(None)` when the search succeeded but no such file exists, and
/// `Err` when the platform's configuration directory could not be determined.
#[cfg(not(unix))]
pub fn find_config_file(relative: &Path) -> Result<Option<PathBuf>, ConfigLocationError> {
    let root = app_data_root()?;
    Ok(find_in_app_data(&root, relative))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn app_data_path_inserts_the_acton_prefix() {
        let path = app_data_config_path(Path::new("/roaming"), Path::new("config.toml"));

        assert_eq!(path, Path::new("/roaming/acton/config.toml"));
    }

    #[test]
    fn app_data_path_preserves_a_nested_relative_path() {
        let path = app_data_config_path(Path::new("/roaming"), &Path::new("myapp").join("ipc.toml"));

        assert_eq!(path, Path::new("/roaming/acton/myapp/ipc.toml"));
    }

    #[test]
    fn app_data_lookup_finds_an_existing_file() {
        let root = TempDir::new().expect("temp dir");
        let dir = root.path().join("acton");
        fs::create_dir_all(&dir).expect("create acton dir");
        fs::write(dir.join("config.toml"), "").expect("write config");

        let found = find_in_app_data(root.path(), Path::new("config.toml"));

        assert_eq!(found, Some(dir.join("config.toml")));
    }

    #[test]
    fn app_data_lookup_reports_a_missing_file_as_none() {
        let root = TempDir::new().expect("temp dir");

        let found = find_in_app_data(root.path(), Path::new("config.toml"));

        assert_eq!(found, None);
    }

    #[test]
    fn app_data_lookup_rejects_a_directory_at_the_config_path() {
        let root = TempDir::new().expect("temp dir");
        fs::create_dir_all(root.path().join("acton").join("config.toml")).expect("create dir");

        let found = find_in_app_data(root.path(), Path::new("config.toml"));

        assert_eq!(
            found, None,
            "a directory named config.toml is not a configuration file"
        );
    }

    /// Pins the `acton` prefix on the shipped Unix path, which the `app_data_*`
    /// tests cannot reach. A typo in [`PREFIX`] would leave every Unix user's
    /// configuration silently unread, and no other test in the crate asserts on
    /// the located path.
    ///
    /// The written file is empty so that a sibling test initialising the global
    /// `CONFIG` during the window still observes defaults; the environment is
    /// restored before returning.
    #[cfg(unix)]
    #[test]
    fn unix_lookup_finds_a_file_under_xdg_config_home() {
        let root = TempDir::new().expect("temp dir");
        let dir = root.path().join("acton");
        fs::create_dir_all(&dir).expect("create acton dir");
        fs::write(dir.join("config.toml"), "").expect("write config");

        let previous = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", root.path());
        let found = find_config_file(Path::new("config.toml"));
        match previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }

        assert_eq!(found, Ok(Some(dir.join("config.toml"))));
    }

    #[test]
    fn app_data_unset_names_the_variable_to_set() {
        let rendered = ConfigLocationError::AppDataUnset.to_string();

        assert!(
            rendered.contains("APPDATA"),
            "message should name the variable, got: {rendered}"
        );
    }

    #[test]
    fn base_directories_error_carries_the_underlying_cause() {
        let rendered =
            ConfigLocationError::BaseDirectories("HOME is not set".to_string()).to_string();

        assert!(
            rendered.contains("HOME is not set"),
            "message should carry the cause, got: {rendered}"
        );
    }
}
