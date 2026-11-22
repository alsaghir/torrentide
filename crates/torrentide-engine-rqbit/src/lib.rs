//! BitTorrent engine for Torrentide, backed by librqbit.

use std::sync::Arc;

use librqbit::{AddTorrent, AddTorrentOptions, Session};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Why [`download`] failed.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// The session rejected the torrent: unreadable source, bad metadata or storage.
    #[error("adding the torrent failed")]
    Add(#[source] BoxError),
    /// `list_only` was set in the options, so nothing was added.
    #[error("the torrent was listed, not added")]
    ListOnly,
    /// The torrent was added but entered the error state.
    #[error("downloading the torrent failed")]
    Transfer(#[source] BoxError),
}

/// Adds `torrent` to `session` and waits until every selected file is on disk.
///
/// A torrent the session already manages is waited on, not added twice. A torrent
/// added paused is waited on until it is resumed.
///
/// # Errors
///
/// [`DownloadError`] if the torrent cannot be added or fails while downloading.
pub async fn download(
    session: &Arc<Session>,
    torrent: AddTorrent<'_>,
    options: AddTorrentOptions,
) -> Result<(), DownloadError> {
    let handle = session
        .add_torrent(torrent, Some(options))
        .await
        .map_err(|e| DownloadError::Add(e.into()))?
        .into_handle()
        .ok_or(DownloadError::ListOnly)?;
    handle
        .wait_until_completed()
        .await
        .map_err(|e| DownloadError::Transfer(e.into()))
}
