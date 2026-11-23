//! Global settings, their TOML form and their file.

mod file;

use std::{num::NonZeroU16, path::PathBuf};

use serde::{Deserialize, Serialize};

pub use file::{LoadedSettings, SettingsFile, SettingsFileError};

/// Format version written to and required from the file; bumped on incompatible changes.
const VERSION: i64 = 1;
const VERSION_KEY: &str = "version";

/// Settings that apply to every torrent unless a torrent overrides them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub downloads: Downloads,
    pub connection: Connection,
    pub bittorrent: BitTorrent,
}

/// Where new torrents are saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Downloads {
    pub dir: PathBuf,
}

/// How peers reach the client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connection {
    pub listen_port: NonZeroU16,
}

/// Peer discovery beyond trackers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BitTorrent {
    pub dht: bool,
}

/// Settings text that cannot be read, or settings that cannot be written as TOML.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("settings are not UTF-8 text")]
    NotUtf8(#[source] std::str::Utf8Error),
    #[error("settings are not valid")]
    Invalid(#[source] toml::de::Error),
    #[error("settings have no `{VERSION_KEY}`")]
    MissingVersion,
    #[error("settings version {0} is not supported, expected {VERSION}")]
    UnsupportedVersion(toml::Value),
    #[error("settings cannot be written as TOML")]
    Unrepresentable(#[source] toml::ser::Error),
}

impl Settings {
    /// First-run settings; the download directory and listen port depend on the machine.
    #[must_use]
    pub const fn with_defaults(download_dir: PathBuf, listen_port: NonZeroU16) -> Self {
        Self {
            downloads: Downloads { dir: download_dir },
            connection: Connection { listen_port },
            bittorrent: BitTorrent { dht: true },
        }
    }

    /// Reads settings written by [`Settings::to_toml`] or edited by the user.
    ///
    /// # Errors
    ///
    /// [`SettingsError`] when the text is not TOML, has another version, or has missing,
    /// unknown or out-of-range keys.
    pub fn from_toml(text: &str) -> Result<Self, SettingsError> {
        let mut table: toml::Table = text.parse().map_err(SettingsError::Invalid)?;
        let version = table
            .remove(VERSION_KEY)
            .ok_or(SettingsError::MissingVersion)?;
        if version.as_integer() != Some(VERSION) {
            return Err(SettingsError::UnsupportedVersion(version));
        }
        table.try_into().map_err(SettingsError::Invalid)
    }

    /// The settings as TOML text, including the format version.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Unrepresentable`] when a path is not valid UTF-8.
    pub fn to_toml(&self) -> Result<String, SettingsError> {
        let mut table = toml::Table::try_from(self).map_err(SettingsError::Unrepresentable)?;
        table.insert(VERSION_KEY.to_owned(), VERSION.into());
        Ok(table.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = r#"
        version = 1

        [downloads]
        dir = "/home/user/Downloads"

        [connection]
        listen_port = 51413

        [bittorrent]
        dht = false
    "#;

    fn settings() -> Settings {
        Settings::with_defaults(
            PathBuf::from("/home/user/Downloads"),
            NonZeroU16::new(51413).expect("non-zero"),
        )
    }

    #[test]
    fn reads_a_settings_document() {
        let mut expected = settings();
        expected.bittorrent.dht = false;

        assert_eq!(Settings::from_toml(DOCUMENT).unwrap(), expected);
    }

    #[test]
    fn written_settings_read_back_unchanged() {
        let text = settings().to_toml().unwrap();

        assert!(text.contains("version = 1"), "{text}");
        assert_eq!(Settings::from_toml(&text).unwrap(), settings());
    }

    #[test]
    fn rejects_a_missing_or_other_version() {
        let missing = DOCUMENT.replace("version = 1", "");
        let other = DOCUMENT.replace("version = 1", "version = 2");

        assert!(matches!(
            Settings::from_toml(&missing),
            Err(SettingsError::MissingVersion)
        ));
        assert!(matches!(
            Settings::from_toml(&other),
            Err(SettingsError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn rejects_unknown_missing_and_out_of_range_keys() {
        let unknown = DOCUMENT.replace("dht = false", "dht = false\npex = true");
        let missing = DOCUMENT.replace("dht = false", "");
        let zero_port = DOCUMENT.replace("51413", "0");

        for text in [unknown, missing, zero_port, "not toml".to_owned()] {
            assert!(
                matches!(Settings::from_toml(&text), Err(SettingsError::Invalid(_))),
                "{text}"
            );
        }
    }
}
