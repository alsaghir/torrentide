//! Torrentide's client logic, independent of the BitTorrent engine and the UI.

mod database;
mod paths;
mod reconcile;
mod settings;

use std::{fmt, path::PathBuf};

pub use database::{Database, DatabaseError, TorrentRecord};
pub use paths::{APP_NAME, HomeNotFound, Paths};
pub use reconcile::{Reconciled, reconcile};
pub use settings::{
    BitTorrent, Connection, Downloads, LoadedSettings, Settings, SettingsError, SettingsFile,
    SettingsFileError,
};

/// An error from a dependency, kept as the `source` of a Torrentide error.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Identifies a torrent by its v1 info hash, which is the same across restarts and engines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TorrentId([u8; 20]);

impl TorrentId {
    #[must_use]
    pub const fn from_info_hash(info_hash: [u8; 20]) -> Self {
        Self(info_hash)
    }

    #[must_use]
    pub const fn info_hash(self) -> [u8; 20] {
        self.0
    }
}

/// Lowercase hex, as in magnet links.
impl fmt::Display for TorrentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// Where a torrent's metadata comes from.
#[derive(Debug)]
pub enum AddSource {
    /// The contents of a `.torrent` file.
    TorrentFile(Vec<u8>),
}

#[derive(Debug)]
pub struct AddOptions {
    /// Directory the torrent's files are written to.
    pub save_path: PathBuf,
    /// Add without downloading or seeding until resumed.
    pub start_paused: bool,
}

/// What happens to a removed torrent's downloaded files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveMode {
    KeepFiles,
    DeleteFiles,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TorrentState {
    /// Verifying the files already on disk.
    Checking,
    Paused,
    /// Downloading or seeding.
    Running,
    /// Stopped by an error; resuming retries.
    Failed(String),
}

/// A torrent as seen at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorrentStatus {
    pub id: TorrentId,
    pub state: TorrentState,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("torrent {0} is already added")]
    AlreadyAdded(TorrentId),
    #[error("no torrent {0}")]
    UnknownTorrent(TorrentId),
    /// A failure specific to the engine implementation.
    #[error("the engine failed")]
    Backend(#[source] BoxError),
}

/// Downloads and seeds torrents.
///
/// Futures are `Send` so callers can run them on a multi-threaded runtime; the engine is
/// `Send + Sync + 'static` so it can be shared with spawned tasks.
pub trait Engine: Send + Sync + 'static {
    /// Adds a torrent and starts it unless [`AddOptions::start_paused`] is set.
    ///
    /// # Errors
    ///
    /// [`EngineError::AlreadyAdded`] if the engine has the torrent already;
    /// [`EngineError::Backend`] if the metadata is invalid or the files cannot be prepared.
    fn add(
        &self,
        source: AddSource,
        options: AddOptions,
    ) -> impl Future<Output = Result<TorrentId, EngineError>> + Send;

    /// Stops downloading and seeding a torrent; does nothing if it is paused already.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownTorrent`] if the engine does not have it;
    /// [`EngineError::Backend`] if the engine cannot pause it in its current state.
    fn pause(&self, id: TorrentId) -> impl Future<Output = Result<(), EngineError>> + Send;

    /// Starts a paused or failed torrent; does nothing if it is running already.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownTorrent`] if the engine does not have it;
    /// [`EngineError::Backend`] if the engine cannot start it.
    fn resume(&self, id: TorrentId) -> impl Future<Output = Result<(), EngineError>> + Send;

    /// Stops a torrent and forgets it.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownTorrent`] if the engine does not have it;
    /// [`EngineError::Backend`] if its files cannot be deleted.
    fn remove(
        &self,
        id: TorrentId,
        mode: RemoveMode,
    ) -> impl Future<Output = Result<(), EngineError>> + Send;

    /// The status of every torrent the engine has, in no particular order.
    fn snapshot(&self) -> Vec<TorrentStatus>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_id_displays_as_lowercase_hex() {
        let mut info_hash = [0; 20];
        info_hash[0] = 0xab;
        info_hash[19] = 0x0f;
        assert_eq!(
            TorrentId::from_info_hash(info_hash).to_string(),
            "ab0000000000000000000000000000000000000f"
        );
    }
}
