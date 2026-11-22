//! BitTorrent engine for Torrentide, backed by librqbit.

use std::{fmt, sync::Arc};

use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, dht::Id20};
use torrentide_core::{
    AddOptions, AddSource, BoxError, Engine, EngineError, RemoveMode, TorrentId,
};

/// [`Engine`] backed by a librqbit [`Session`].
pub struct RqbitEngine {
    session: Arc<Session>,
}

impl RqbitEngine {
    #[must_use]
    pub const fn new(session: Arc<Session>) -> Self {
        Self { session }
    }
}

impl fmt::Debug for RqbitEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RqbitEngine").finish_non_exhaustive()
    }
}

impl Engine for RqbitEngine {
    async fn add(&self, source: AddSource, options: AddOptions) -> Result<TorrentId, EngineError> {
        let AddSource::TorrentFile(bytes) = source;
        let output_folder = options.save_path.to_str().ok_or_else(|| {
            EngineError::Backend(
                format!(
                    "librqbit needs a UTF-8 save path: {}",
                    options.save_path.display()
                )
                .into(),
            )
        })?;
        let response = self
            .session
            .add_torrent(
                AddTorrent::from_bytes(bytes),
                Some(AddTorrentOptions {
                    paused: options.start_paused,
                    output_folder: Some(output_folder.to_owned()),
                    // Reuse files already on disk after a hash check instead of refusing them
                    // (librqbit's default); librqbit does the same when restoring its session.
                    overwrite: true,
                    ..AddTorrentOptions::default()
                }),
            )
            .await
            .map_err(|e| EngineError::Backend(e.into()))?;
        match response {
            AddTorrentResponse::Added(_, handle) => {
                Ok(TorrentId::from_info_hash(handle.info_hash().0))
            }
            AddTorrentResponse::AlreadyManaged(_, handle) => Err(EngineError::AlreadyAdded(
                TorrentId::from_info_hash(handle.info_hash().0),
            )),
            AddTorrentResponse::ListOnly(_) => unreachable!("`list_only` is never set"),
        }
    }

    async fn remove(&self, id: TorrentId, mode: RemoveMode) -> Result<(), EngineError> {
        let hash = Id20::new(id.info_hash());
        // librqbit reports an unknown torrent only as an untyped error, so look it up first.
        if self.session.get(hash.into()).is_none() {
            return Err(EngineError::UnknownTorrent(id));
        }
        self.session
            .delete(hash.into(), mode == RemoveMode::DeleteFiles)
            .await
            .map_err(|e| EngineError::Backend(e.into()))
    }
}

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
