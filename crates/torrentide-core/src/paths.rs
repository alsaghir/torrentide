//! Where the client keeps its files.

use std::path::{Path, PathBuf};

/// Directory name under each base directory.
pub const APP_NAME: &str = "torrentide";

/// The home directory, needed to resolve the standard directories, could not be determined.
#[derive(Debug, thiserror::Error)]
#[error("home directory not found")]
pub struct HomeNotFound;

/// The client's directories, resolved once at startup and passed to everything that stores files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
}

impl Paths {
    /// The platform's standard per-user directories, e.g. `$XDG_CONFIG_HOME/torrentide` on Linux.
    ///
    /// # Errors
    ///
    /// [`HomeNotFound`] when the home directory cannot be determined.
    pub fn standard() -> Result<Self, HomeNotFound> {
        let dirs = directories::ProjectDirs::from("", "", APP_NAME).ok_or(HomeNotFound)?;
        Ok(Self {
            config: dirs.config_dir().to_owned(),
            data: dirs.data_dir().to_owned(),
            // `None` only on platforms without a state directory; data is the closest fit there.
            state: dirs.state_dir().unwrap_or(dirs.data_dir()).to_owned(),
        })
    }

    /// Every directory under `root`, for portable installs and tests.
    #[must_use]
    pub fn portable(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
        }
    }

    /// User-edited configuration, such as the settings file.
    #[must_use]
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// State the client must keep, such as the torrent database.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// State the client can lose without harm, such as logs.
    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs(paths: &Paths) -> [&Path; 3] {
        [paths.config_dir(), paths.data_dir(), paths.state_dir()]
    }

    #[test]
    fn standard_directories_are_absolute_distinct_and_named_after_the_app() {
        let paths = Paths::standard().expect("tests run with a home directory");

        for dir in dirs(&paths) {
            assert!(dir.is_absolute(), "{dir:?}");
            assert!(dir.ends_with(APP_NAME), "{dir:?}");
        }
        let [config, data, state] = dirs(&paths);
        assert!(config != data && data != state && config != state);
    }

    #[test]
    fn portable_directories_are_distinct_and_under_the_root() {
        let root = Path::new("/portable/torrentide");
        let paths = Paths::portable(root);

        for dir in dirs(&paths) {
            assert!(dir.starts_with(root), "{dir:?}");
        }
        let [config, data, state] = dirs(&paths);
        assert!(config != data && data != state && config != state);
    }
}
