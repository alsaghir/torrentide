//! `RqbitEngine` against an isolated session: no network, the payload is already on disk.

use std::{fs, path::Path, time::Duration};

use torrentide_core::{
    AddOptions, AddSource, Engine, EngineError, Paths, RemoveMode, Settings, TorrentId,
    TorrentState, TorrentStatus,
};
use torrentide_engine_rqbit::RqbitEngine;
use torrentide_testkit::{PAYLOAD_NAME, isolated_session, payload_torrent};

struct Fixture {
    engine: RqbitEngine,
    torrent_file: Vec<u8>,
    id: TorrentId,
    dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let (_, torrent) = payload_torrent(dir.path()).await;
    Fixture {
        engine: RqbitEngine::new(isolated_session(dir.path(), None).await),
        torrent_file: torrent.as_bytes().unwrap().to_vec(),
        id: TorrentId::from_info_hash(torrent.info_hash().0),
        dir,
    }
}

fn add_paused(fixture: &Fixture) -> impl Future<Output = Result<TorrentId, EngineError>> {
    fixture.engine.add(
        AddSource::TorrentFile(fixture.torrent_file.clone()),
        AddOptions {
            save_path: fixture.dir.path().to_owned(),
            start_paused: true,
        },
    )
}

fn payload_exists(dir: &Path) -> bool {
    dir.join(PAYLOAD_NAME).try_exists().unwrap()
}

fn state(engine: &RqbitEngine, id: TorrentId) -> Option<TorrentState> {
    engine
        .snapshot()
        .into_iter()
        .find(|status| status.id == id)
        .map(|status| status.state)
}

/// Polls until the torrent reaches `expected`: the hash check runs in the background.
// TODO: wait on engine events instead of polling once the engine reports them.
async fn wait_for_state(engine: &RqbitEngine, id: TorrentId, expected: TorrentState) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while state(engine, id).as_ref() != Some(&expected) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {expected:?}, got {:?}", state(engine, id)));
}

#[tokio::test]
async fn add_returns_the_info_hash() {
    let fixture = fixture().await;

    assert_eq!(add_paused(&fixture).await.unwrap(), fixture.id);
}

#[tokio::test]
async fn torrent_id_is_the_id_add_returns() {
    let fixture = fixture().await;

    let id = fixture
        .engine
        .torrent_id(&AddSource::TorrentFile(fixture.torrent_file.clone()))
        .unwrap();

    assert_eq!(fixture.engine.snapshot(), Vec::new());
    assert_eq!(id, fixture.id);
    assert_eq!(add_paused(&fixture).await.unwrap(), id);
}

#[tokio::test]
async fn torrent_id_rejects_invalid_metadata() {
    let fixture = fixture().await;

    let error = fixture
        .engine
        .torrent_id(&AddSource::TorrentFile(b"not bencode".to_vec()))
        .unwrap_err();

    assert!(matches!(error, EngineError::Backend(_)));
}

#[tokio::test]
async fn adding_a_torrent_twice_is_rejected() {
    let fixture = fixture().await;
    add_paused(&fixture).await.unwrap();

    let error = add_paused(&fixture).await.unwrap_err();

    assert!(matches!(error, EngineError::AlreadyAdded(id) if id == fixture.id));
}

#[tokio::test]
async fn removing_an_unknown_torrent_is_rejected() {
    let fixture = fixture().await;

    let error = fixture
        .engine
        .remove(fixture.id, RemoveMode::KeepFiles)
        .await
        .unwrap_err();

    assert!(matches!(error, EngineError::UnknownTorrent(id) if id == fixture.id));
}

#[tokio::test]
async fn remove_keeping_files_forgets_the_torrent_only() {
    let fixture = fixture().await;
    add_paused(&fixture).await.unwrap();

    fixture
        .engine
        .remove(fixture.id, RemoveMode::KeepFiles)
        .await
        .unwrap();

    assert!(payload_exists(fixture.dir.path()));
    assert_eq!(add_paused(&fixture).await.unwrap(), fixture.id);
}

#[tokio::test]
async fn remove_deleting_files_deletes_the_payload() {
    let fixture = fixture().await;
    add_paused(&fixture).await.unwrap();

    fixture
        .engine
        .remove(fixture.id, RemoveMode::DeleteFiles)
        .await
        .unwrap();

    assert!(!payload_exists(fixture.dir.path()));
}

#[tokio::test]
async fn torrent_added_paused_is_checked_then_paused() {
    let fixture = fixture().await;

    add_paused(&fixture).await.unwrap();

    wait_for_state(&fixture.engine, fixture.id, TorrentState::Paused).await;
}

#[tokio::test]
async fn resume_runs_and_pause_stops_the_torrent() {
    let fixture = fixture().await;
    add_paused(&fixture).await.unwrap();
    wait_for_state(&fixture.engine, fixture.id, TorrentState::Paused).await;

    fixture.engine.resume(fixture.id).await.unwrap();
    wait_for_state(&fixture.engine, fixture.id, TorrentState::Running).await;

    fixture.engine.pause(fixture.id).await.unwrap();
    wait_for_state(&fixture.engine, fixture.id, TorrentState::Paused).await;
}

#[tokio::test]
async fn pause_and_resume_are_idempotent() {
    let fixture = fixture().await;
    add_paused(&fixture).await.unwrap();
    wait_for_state(&fixture.engine, fixture.id, TorrentState::Paused).await;

    fixture.engine.pause(fixture.id).await.unwrap();
    assert_eq!(
        state(&fixture.engine, fixture.id),
        Some(TorrentState::Paused)
    );

    fixture.engine.resume(fixture.id).await.unwrap();
    wait_for_state(&fixture.engine, fixture.id, TorrentState::Running).await;
    fixture.engine.resume(fixture.id).await.unwrap();
    assert_eq!(
        state(&fixture.engine, fixture.id),
        Some(TorrentState::Running)
    );
}

#[tokio::test]
async fn pausing_or_resuming_an_unknown_torrent_is_rejected() {
    let fixture = fixture().await;

    let pause = fixture.engine.pause(fixture.id).await.unwrap_err();
    let resume = fixture.engine.resume(fixture.id).await.unwrap_err();

    assert!(matches!(pause, EngineError::UnknownTorrent(id) if id == fixture.id));
    assert!(matches!(resume, EngineError::UnknownTorrent(id) if id == fixture.id));
}

fn test_setup() -> (tempfile::TempDir, Paths, Settings) {
    let root = tempfile::tempdir().expect("temp dir");
    let paths = Paths::portable(root.path());
    let mut settings = Settings::first_run(&paths);
    settings.downloads.dir = paths.data_dir().join("downloads");
    settings.bittorrent.dht = false;
    settings.bittorrent.lsd = false;
    fs::create_dir_all(&settings.downloads.dir).expect("downloads dir created");
    (root, paths, settings)
}

#[tokio::test]
async fn restart_restores_the_torrents() {
    let (_root, paths, settings) = test_setup();
    let (_, torrent) = payload_torrent(&settings.downloads.dir).await;
    let torrent_file = torrent.as_bytes().expect("torrent bytes").to_vec();
    let expected_id = TorrentId::from_info_hash(torrent.info_hash().0);

    let engine = RqbitEngine::start(&paths, &settings)
        .await
        .expect("engine starts");
    let id = engine
        .add(
            AddSource::TorrentFile(torrent_file),
            AddOptions {
                save_path: settings.downloads.dir.clone(),
                start_paused: true,
            },
        )
        .await
        .expect("torrent added");
    assert_eq!(id, expected_id);
    wait_for_state(&engine, id, TorrentState::Paused).await;

    engine.stop().await;

    let engine = RqbitEngine::start(&paths, &settings)
        .await
        .expect("engine restarts");
    wait_for_state(&engine, id, TorrentState::Paused).await;
    let snapshot = engine.snapshot();
    let restored = snapshot.into_iter().find(|status| status.id == id);

    assert_eq!(
        restored,
        Some(TorrentStatus {
            id,
            state: TorrentState::Paused,
        })
    );
    engine.stop().await;
}

#[tokio::test]
async fn removed_torrent_stays_removed_after_restart() {
    let (_root, paths, settings) = test_setup();
    let (_, torrent) = payload_torrent(&settings.downloads.dir).await;
    let torrent_file = torrent.as_bytes().expect("torrent bytes").to_vec();
    let expected_id = TorrentId::from_info_hash(torrent.info_hash().0);

    let engine = RqbitEngine::start(&paths, &settings)
        .await
        .expect("engine starts");
    let id = engine
        .add(
            AddSource::TorrentFile(torrent_file),
            AddOptions {
                save_path: settings.downloads.dir.clone(),
                start_paused: true,
            },
        )
        .await
        .expect("torrent added");
    assert_eq!(id, expected_id);

    engine
        .remove(id, RemoveMode::KeepFiles)
        .await
        .expect("torrent removed");
    assert_eq!(engine.snapshot(), []);

    engine.stop().await;

    let engine = RqbitEngine::start(&paths, &settings)
        .await
        .expect("engine restarts");
    assert_eq!(engine.snapshot(), []);
    engine.stop().await;
}
