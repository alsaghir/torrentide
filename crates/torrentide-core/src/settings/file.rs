//! The settings file: first-run defaults, atomic saves and recovery from a broken file.

use std::{
    fs,
    io::{self, Write as _},
    num::NonZeroU16,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{Settings, SettingsError};
use crate::Paths;

const FILE_NAME: &str = "settings.toml";

/// Start of the IANA dynamic port range; the default port stays clear of registered services.
const DYNAMIC_PORTS_START: NonZeroU16 = NonZeroU16::new(49152).unwrap();

/// The settings file in the config directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsFile {
    dir: PathBuf,
    path: PathBuf,
}

/// Settings from [`SettingsFile::load_or_init`], and how they were obtained.
#[derive(Debug)]
pub enum LoadedSettings {
    /// Read from the file.
    Existing(Settings),
    /// There was no file; the defaults were written.
    Created(Settings),
    /// The file did not hold valid settings; it was moved to `invalid` and the defaults were written.
    Recovered {
        settings: Settings,
        invalid: PathBuf,
        reason: SettingsError,
    },
}

/// The settings file cannot be read or written.
#[derive(Debug, thiserror::Error)]
pub enum SettingsFileError {
    #[error("cannot read {}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot write {}", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("settings cannot be saved")]
    Encode(#[source] SettingsError),
}

impl Settings {
    /// Defaults for a new install: the user's download folder and a random port in the dynamic range.
    #[must_use]
    pub fn first_run(paths: &Paths) -> Self {
        let download_dir = match directories::UserDirs::new() {
            Some(dirs) => dirs
                .download_dir()
                .map_or_else(|| dirs.home_dir().join("Downloads"), Path::to_path_buf),
            None => paths.data_dir().join("downloads"),
        };
        let offset = rand::random_range(0..=u16::MAX - DYNAMIC_PORTS_START.get());
        Self::with_defaults(download_dir, DYNAMIC_PORTS_START.saturating_add(offset))
    }
}

impl LoadedSettings {
    #[must_use]
    pub fn into_settings(self) -> Settings {
        match self {
            Self::Existing(settings)
            | Self::Created(settings)
            | Self::Recovered { settings, .. } => settings,
        }
    }
}

impl SettingsFile {
    #[must_use]
    pub fn new(paths: &Paths) -> Self {
        let dir = paths.config_dir().to_owned();
        let path = dir.join(FILE_NAME);
        Self { dir, path }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the settings; writes `defaults()` when the file is missing or broken.
    ///
    /// A broken file is kept as `settings.toml.invalid-<unix seconds>` for the user to inspect.
    ///
    /// # Errors
    ///
    /// [`SettingsFileError`] when the file exists but cannot be read, or the defaults cannot be
    /// written.
    pub fn load_or_init(
        &self,
        defaults: impl FnOnce() -> Settings,
    ) -> Result<LoadedSettings, SettingsFileError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let settings = defaults();
                self.save(&settings)?;
                return Ok(LoadedSettings::Created(settings));
            }
            Err(source) => {
                return Err(SettingsFileError::Read {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let parsed = str::from_utf8(&bytes)
            .map_err(SettingsError::NotUtf8)
            .and_then(Settings::from_toml);
        match parsed {
            Ok(settings) => Ok(LoadedSettings::Existing(settings)),
            Err(reason) => {
                let invalid = self.invalid_path();
                fs::rename(&self.path, &invalid).map_err(|source| SettingsFileError::Write {
                    path: invalid.clone(),
                    source,
                })?;
                let settings = defaults();
                self.save(&settings)?;
                Ok(LoadedSettings::Recovered {
                    settings,
                    invalid,
                    reason,
                })
            }
        }
    }

    /// Replaces the file atomically: readers see the old or the new settings, never a partial file.
    ///
    /// # Errors
    ///
    /// [`SettingsFileError`] when the settings are not representable as TOML or the file cannot be
    /// written.
    pub fn save(&self, settings: &Settings) -> Result<(), SettingsFileError> {
        let text = settings.to_toml().map_err(SettingsFileError::Encode)?;
        let write_error = |source: io::Error| SettingsFileError::Write {
            path: self.path.clone(),
            source,
        };
        fs::create_dir_all(&self.dir).map_err(write_error)?;
        // The temporary file sits in the same directory so the rename stays on one file system.
        let mut temp = tempfile::NamedTempFile::new_in(&self.dir).map_err(write_error)?;
        temp.write_all(text.as_bytes()).map_err(write_error)?;
        // Flushed before the rename, so a crash cannot leave an empty file under the real name.
        temp.as_file().sync_all().map_err(write_error)?;
        temp.persist(&self.path)
            .map_err(|error| write_error(error.error))?;
        Ok(())
    }

    fn invalid_path(&self) -> PathBuf {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".invalid-{seconds}"));
        name.into()
    }
}
