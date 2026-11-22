//! BitTorrent engine for Torrentide, backed by librqbit.

use std::{fmt, sync::Arc};

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent, Session, TorrentStats,
    TorrentStatsState, dht::Id20,
};
use torrentide_core::{
    AddOptions, AddSource, BoxError, Engine, EngineError, RemoveMode, TorrentId, TorrentState,
    TorrentStatus,
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

    fn torrent(&self, id: TorrentId) -> Result<Arc<ManagedTorrent>, EngineError> {
        self.session
            .get(Id20::new(id.info_hash()).into())
            .ok_or(EngineError::UnknownTorrent(id))
    }
}

impl fmt::Debug for RqbitEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RqbitEngine").finish_non_exhaustive()
    }
}

fn torrent_id(torrent: &ManagedTorrent) -> TorrentId {
    TorrentId::from_info_hash(torrent.info_hash().0)
}

fn torrent_state(stats: TorrentStats) -> TorrentState {
    match stats.state {
        TorrentStatsState::Initializing { .. } => TorrentState::Checking,
        TorrentStatsState::Paused => TorrentState::Paused,
        TorrentStatsState::Live => TorrentState::Running,
        TorrentStatsState::Error => TorrentState::Failed(stats.error.unwrap_or_default()),
    }
}

fn backend(error: impl Into<BoxError>) -> EngineError {
    EngineError::Backend(error.into())
}

impl Engine for RqbitEngine {
    async fn add(&self, source: AddSource, options: AddOptions) -> Result<TorrentId, EngineError> {
        let AddSource::TorrentFile(bytes) = source;
        let output_folder = options.save_path.to_str().ok_or_else(|| {
            backend(format!(
                "librqbit needs a UTF-8 save path: {}",
                options.save_path.display()
            ))
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
            .map_err(backend)?;
        match response {
            AddTorrentResponse::Added(_, torrent) => Ok(torrent_id(&torrent)),
            AddTorrentResponse::AlreadyManaged(_, torrent) => {
                Err(EngineError::AlreadyAdded(torrent_id(&torrent)))
            }
            AddTorrentResponse::ListOnly(_) => unreachable!("`list_only` is never set"),
        }
    }

    async fn pause(&self, id: TorrentId) -> Result<(), EngineError> {
        let torrent = self.torrent(id)?;
        // librqbit rejects pausing twice. `is_paused` is the requested state, so it also
        // covers a check that will pause when it finishes.
        if torrent.is_paused() {
            return Ok(());
        }
        self.session.pause(&torrent).await.map_err(backend)
    }

    async fn resume(&self, id: TorrentId) -> Result<(), EngineError> {
        let torrent = self.torrent(id)?;
        // librqbit rejects resuming a live torrent.
        if matches!(torrent.stats().state, TorrentStatsState::Live) {
            return Ok(());
        }
        self.session.unpause(&torrent).await.map_err(backend)
    }

    async fn remove(&self, id: TorrentId, mode: RemoveMode) -> Result<(), EngineError> {
        // librqbit reports an unknown torrent only as an untyped error, so look it up first.
        let torrent = self.torrent(id)?;
        self.session
            .delete(torrent.id().into(), mode == RemoveMode::DeleteFiles)
            .await
            .map_err(backend)
    }

    fn snapshot(&self) -> Vec<TorrentStatus> {
        self.session.with_torrents(|torrents| {
            torrents
                .map(|(_, torrent)| TorrentStatus {
                    id: torrent_id(torrent),
                    state: torrent_state(torrent.stats()),
                })
                .collect()
        })
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
