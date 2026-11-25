//! The client actor: publishing an engine's torrents and forwarding calls to it.

use std::{
    future::{Ready, ready},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::time::{self, Instant};
use torrentide_core::{
    AddOptions, AddSource, ClientHandle, Engine, EngineError, RemoveMode, TorrentId, TorrentState,
    TorrentStatus,
};

const TICK: Duration = Duration::from_secs(1);

type Torrents = Arc<Mutex<Vec<TorrentStatus>>>;

/// An engine over a list the test can also change directly.
///
/// A torrent file is one byte `b`; its id is `b` repeated.
struct SharedEngine(Torrents);

impl SharedEngine {
    fn set_state(&self, id: TorrentId, state: TorrentState) -> Ready<Result<(), EngineError>> {
        let mut torrents = self.0.lock().unwrap();
        ready(match torrents.iter_mut().find(|torrent| torrent.id == id) {
            Some(torrent) => {
                torrent.state = state;
                Ok(())
            }
            None => Err(EngineError::UnknownTorrent(id)),
        })
    }
}

impl Engine for SharedEngine {
    fn torrent_id(&self, source: &AddSource) -> Result<TorrentId, EngineError> {
        let AddSource::TorrentFile(bytes) = source;
        Ok(TorrentId::from_info_hash([bytes[0]; 20]))
    }

    fn add(
        &self,
        source: AddSource,
        options: AddOptions,
    ) -> impl Future<Output = Result<TorrentId, EngineError>> {
        let id = match self.torrent_id(&source) {
            Ok(id) => id,
            Err(error) => return ready(Err(error)),
        };
        let mut torrents = self.0.lock().unwrap();
        ready(if torrents.iter().any(|torrent| torrent.id == id) {
            Err(EngineError::AlreadyAdded(id))
        } else {
            let state = if options.start_paused {
                TorrentState::Paused
            } else {
                TorrentState::Running
            };
            torrents.push(TorrentStatus { id, state });
            Ok(id)
        })
    }

    fn pause(&self, id: TorrentId) -> impl Future<Output = Result<(), EngineError>> {
        self.set_state(id, TorrentState::Paused)
    }

    fn resume(&self, id: TorrentId) -> impl Future<Output = Result<(), EngineError>> {
        self.set_state(id, TorrentState::Running)
    }

    fn remove(
        &self,
        id: TorrentId,
        _: RemoveMode,
    ) -> impl Future<Output = Result<(), EngineError>> {
        let mut torrents = self.0.lock().unwrap();
        let before = torrents.len();
        torrents.retain(|torrent| torrent.id != id);
        ready(if torrents.len() < before {
            Ok(())
        } else {
            Err(EngineError::UnknownTorrent(id))
        })
    }

    fn snapshot(&self) -> Vec<TorrentStatus> {
        self.0.lock().unwrap().clone()
    }
}

fn id(byte: u8) -> TorrentId {
    TorrentId::from_info_hash([byte; 20])
}

fn status(byte: u8, state: TorrentState) -> TorrentStatus {
    TorrentStatus {
        id: id(byte),
        state,
    }
}

fn spawn(torrents: Vec<TorrentStatus>) -> (Torrents, ClientHandle<SharedEngine>) {
    let shared = Arc::new(Mutex::new(torrents));
    let client = ClientHandle::spawn(SharedEngine(Arc::clone(&shared)), TICK);
    (shared, client)
}

fn published(client: &ClientHandle<SharedEngine>) -> Vec<TorrentStatus> {
    client.snapshot().borrow().clone()
}

#[tokio::test(start_paused = true)]
async fn the_snapshot_starts_with_the_engine_torrents() {
    let (_, client) = spawn(vec![status(1, TorrentState::Running)]);

    assert_eq!(published(&client), vec![status(1, TorrentState::Running)]);
}

#[tokio::test(start_paused = true)]
async fn a_changed_engine_list_is_published_on_the_next_tick() {
    let (shared, client) = spawn(vec![status(1, TorrentState::Running)]);
    let mut snapshot = client.snapshot();
    let start = Instant::now();

    shared
        .lock()
        .unwrap()
        .push(status(2, TorrentState::Running));

    time::timeout(TICK * 2, snapshot.changed())
        .await
        .expect("published within two ticks")
        .unwrap();
    assert_eq!(start.elapsed(), TICK);
    assert_eq!(
        *snapshot.borrow(),
        vec![
            status(1, TorrentState::Running),
            status(2, TorrentState::Running)
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn an_unchanged_engine_list_wakes_no_one() {
    let (_, client) = spawn(vec![status(1, TorrentState::Running)]);
    let mut snapshot = client.snapshot();

    let woken = time::timeout(TICK * 3, snapshot.changed()).await;

    assert!(woken.is_err(), "woken by an unchanged list");
}

#[tokio::test(start_paused = true)]
async fn an_added_torrent_is_published_before_add_returns() {
    let (_, client) = spawn(Vec::new());
    let start = Instant::now();

    let added = client
        .add(
            AddSource::TorrentFile(vec![1]),
            AddOptions {
                save_path: PathBuf::new(),
                start_paused: true,
            },
        )
        .await
        .unwrap();

    assert_eq!(added, id(1));
    assert_eq!(start.elapsed(), Duration::ZERO, "no tick was needed");
    assert_eq!(published(&client), vec![status(1, TorrentState::Paused)]);
}

#[tokio::test(start_paused = true)]
async fn pause_and_resume_are_published_before_they_return() {
    let (_, client) = spawn(vec![status(1, TorrentState::Running)]);

    client.pause(id(1)).await.unwrap();
    assert_eq!(published(&client), vec![status(1, TorrentState::Paused)]);

    client.resume(id(1)).await.unwrap();
    assert_eq!(published(&client), vec![status(1, TorrentState::Running)]);
}

#[tokio::test(start_paused = true)]
async fn a_removed_torrent_is_gone_from_the_snapshot_when_remove_returns() {
    let (_, client) = spawn(vec![
        status(1, TorrentState::Running),
        status(2, TorrentState::Running),
    ]);

    client.remove(id(1), RemoveMode::KeepFiles).await.unwrap();

    assert_eq!(published(&client), vec![status(2, TorrentState::Running)]);
}

#[tokio::test(start_paused = true)]
async fn engine_errors_reach_the_caller() {
    let (_, client) = spawn(Vec::new());

    let error = client.pause(id(9)).await.unwrap_err();

    assert!(matches!(error, EngineError::UnknownTorrent(unknown) if unknown == id(9)));
}

#[tokio::test(start_paused = true)]
async fn shutdown_returns_the_engine() {
    let (_, client) = spawn(vec![status(1, TorrentState::Running)]);

    let engine = client.shutdown().await;

    assert_eq!(engine.snapshot(), vec![status(1, TorrentState::Running)]);
}
