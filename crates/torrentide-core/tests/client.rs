//! The client actor: publishing an engine's torrents and forwarding calls to it.

use std::{
    future::{Ready, ready},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::time::{self, Instant};
use torrentide_core::{
    AddOptions, AddSource, ClientError, ClientHandle, Database, Engine, EngineError, Paths,
    RemoveMode, TorrentId, TorrentState, TorrentStatus,
};

const TICK: Duration = Duration::from_secs(1);
/// A save path [`SharedEngine`] cannot add to.
const UNWRITABLE: &str = "unwritable";

type Torrents = Arc<Mutex<Vec<TorrentStatus>>>;

/// An engine over a list the test can also change directly.
///
// A torrent file is one byte `b`; its id is `b` repeated. Adding to [`UNWRITABLE`] fails.
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
        } else if options.save_path == Path::new(UNWRITABLE) {
            Err(EngineError::Backend("the save path is unwritable".into()))
        } else {
            let state = if options.start_paused {
                TorrentState::Paused
            } else {
                TorrentState::Running
            };
            torrents.push(status_for(id, state));
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
    status_for(id(byte), state)
}

fn status_for(id: TorrentId, state: TorrentState) -> TorrentStatus {
    TorrentStatus {
        id,
        state,
        name: None,
        size: 0,
        done: 0,
        download_rate: 0,
        upload_rate: 0,
    }
}

fn torrent_file(byte: u8) -> AddSource {
    AddSource::TorrentFile(vec![byte])
}

fn options(save_path: &str) -> AddOptions {
    AddOptions {
        save_path: PathBuf::from(save_path),
        start_paused: true,
    }
}

struct Fixture {
    torrents: Torrents,
    client: ClientHandle<SharedEngine>,
    paths: Paths,
    _root: tempfile::TempDir,
}

impl Fixture {
    /// The torrent rows on disk, read through a second connection.
    fn rows(&self) -> Vec<TorrentId> {
        Database::open(&self.paths)
            .unwrap()
            .torrents()
            .unwrap()
            .into_iter()
            .map(|record| record.id)
            .collect()
    }

    fn published(&self) -> Vec<TorrentStatus> {
        self.client.snapshot().borrow().clone()
    }
}

/// Spawns a client whose engine has `torrents` and whose database has a row for each of `rows`.
fn spawn_with_rows(torrents: Vec<TorrentStatus>, rows: &[TorrentId]) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let paths = Paths::portable(root.path());
    let database = Database::open(&paths).unwrap();
    for &row in rows {
        database.insert_torrent(row, 0).unwrap();
    }
    let shared = Arc::new(Mutex::new(torrents));
    let client = ClientHandle::spawn(SharedEngine(Arc::clone(&shared)), database, TICK).unwrap();
    Fixture {
        torrents: shared,
        client,
        paths,
        _root: root,
    }
}

fn spawn(torrents: Vec<TorrentStatus>) -> Fixture {
    spawn_with_rows(torrents, &[])
}

#[tokio::test(start_paused = true)]
async fn the_snapshot_starts_with_the_engine_torrents() {
    let fixture = spawn(vec![status(1, TorrentState::Running)]);

    assert_eq!(fixture.published(), vec![status(1, TorrentState::Running)]);
}

#[tokio::test(start_paused = true)]
async fn spawn_gives_each_engine_torrent_a_row_and_drops_stale_rows() {
    let fixture = spawn_with_rows(vec![status(1, TorrentState::Running)], &[id(2)]);

    assert_eq!(fixture.rows(), vec![id(1)]);
}

#[tokio::test(start_paused = true)]
async fn a_changed_engine_list_is_published_on_the_next_tick() {
    let fixture = spawn(vec![status(1, TorrentState::Running)]);
    let mut snapshot = fixture.client.snapshot();
    let start = Instant::now();

    fixture
        .torrents
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
    let fixture = spawn(vec![status(1, TorrentState::Running)]);
    let mut snapshot = fixture.client.snapshot();

    let woken = time::timeout(TICK * 3, snapshot.changed()).await;

    assert!(woken.is_err(), "woken by an unchanged list");
}

#[tokio::test(start_paused = true)]
async fn an_added_torrent_is_published_before_add_returns() {
    let fixture = spawn(Vec::new());
    let start = Instant::now();

    let added = fixture
        .client
        .add(torrent_file(1), options(""))
        .await
        .unwrap();

    assert_eq!(added, id(1));
    assert_eq!(start.elapsed(), Duration::ZERO, "no tick was needed");
    assert_eq!(fixture.published(), vec![status(1, TorrentState::Paused)]);
}

#[tokio::test(start_paused = true)]
async fn an_added_torrent_has_a_row_when_add_returns() {
    let fixture = spawn(Vec::new());

    fixture
        .client
        .add(torrent_file(1), options(""))
        .await
        .unwrap();

    assert_eq!(fixture.rows(), vec![id(1)]);
}

#[tokio::test(start_paused = true)]
async fn a_failed_add_leaves_no_row() {
    let fixture = spawn(Vec::new());

    let error = fixture
        .client
        .add(torrent_file(1), options(UNWRITABLE))
        .await
        .unwrap_err();

    assert!(
        matches!(error, ClientError::Engine(EngineError::Backend(_))),
        "{error:?}"
    );
    assert_eq!(fixture.rows(), Vec::new());
}

#[tokio::test(start_paused = true)]
async fn adding_a_torrent_the_engine_has_keeps_its_row() {
    let fixture = spawn(Vec::new());
    // Added behind the client's back, so it has no row yet.
    fixture
        .torrents
        .lock()
        .unwrap()
        .push(status(1, TorrentState::Running));

    let error = fixture
        .client
        .add(torrent_file(1), options(""))
        .await
        .unwrap_err();

    assert!(
        matches!(error, ClientError::Engine(EngineError::AlreadyAdded(known)) if known == id(1)),
        "{error:?}"
    );
    assert_eq!(fixture.rows(), vec![id(1)]);
}

#[tokio::test(start_paused = true)]
async fn pause_and_resume_are_published_before_they_return() {
    let fixture = spawn(vec![status(1, TorrentState::Running)]);

    fixture.client.pause(id(1)).await.unwrap();
    assert_eq!(fixture.published(), vec![status(1, TorrentState::Paused)]);

    fixture.client.resume(id(1)).await.unwrap();
    assert_eq!(fixture.published(), vec![status(1, TorrentState::Running)]);
}

#[tokio::test(start_paused = true)]
async fn a_removed_torrent_is_gone_from_the_snapshot_when_remove_returns() {
    let fixture = spawn(vec![
        status(1, TorrentState::Running),
        status(2, TorrentState::Running),
    ]);

    fixture
        .client
        .remove(id(1), RemoveMode::KeepFiles)
        .await
        .unwrap();

    assert_eq!(fixture.published(), vec![status(2, TorrentState::Running)]);
}

#[tokio::test(start_paused = true)]
async fn a_removed_torrent_loses_its_row() {
    let fixture = spawn(vec![
        status(1, TorrentState::Running),
        status(2, TorrentState::Running),
    ]);

    fixture
        .client
        .remove(id(1), RemoveMode::KeepFiles)
        .await
        .unwrap();

    assert_eq!(fixture.rows(), vec![id(2)]);
}

#[tokio::test(start_paused = true)]
async fn engine_errors_reach_the_caller() {
    let fixture = spawn(Vec::new());

    let error = fixture.client.pause(id(9)).await.unwrap_err();

    assert!(
        matches!(error, ClientError::Engine(EngineError::UnknownTorrent(unknown)) if unknown == id(9)),
        "{error:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_returns_the_engine() {
    let fixture = spawn(vec![status(1, TorrentState::Running)]);

    let engine = fixture.client.shutdown().await;

    assert_eq!(engine.snapshot(), vec![status(1, TorrentState::Running)]);
}
