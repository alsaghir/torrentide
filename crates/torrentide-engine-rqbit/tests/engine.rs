//! `RqbitEngine` against an isolated session: no network, the payload is already on disk.

mod common;

use std::{path::Path, time::Duration};

use common::{PAYLOAD_NAME, isolated_session, payload_torrent};
use torrentide_core::{
    AddOptions, AddSource, Engine, EngineError, RemoveMode, TorrentId, TorrentState,
};
use torrentide_engine_rqbit::RqbitEngine;

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

fn state(fixture: &Fixture) -> Option<TorrentState> {
    fixture
        .engine
        .snapshot()
        .into_iter()
        .find(|status| status.id == fixture.id)
        .map(|status| status.state)
}

/// Polls until the torrent reaches `expected`: the hash check runs in the background.
// TODO(E1 events): wait on engine events instead of polling.
async fn wait_for_state(fixture: &Fixture, expected: TorrentState) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while state(fixture).as_ref() != Some(&expected) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected {expected:?}, got {:?}", state(fixture)));
}

#[tokio::test]
async fn add_returns_the_info_hash() {
    let fixture = fixture().await;

    assert_eq!(add_paused(&fixture).await.unwrap(), fixture.id);
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

    wait_for_state(&fixture, TorrentState::Paused).await;
}

#[tokio::test]
async fn resume_runs_and_pause_stops_the_torrent() {
    let fixture = fixture().await;
    add_paused(&fixture).await.unwrap();
    wait_for_state(&fixture, TorrentState::Paused).await;

    fixture.engine.resume(fixture.id).await.unwrap();
    wait_for_state(&fixture, TorrentState::Running).await;

    fixture.engine.pause(fixture.id).await.unwrap();
    wait_for_state(&fixture, TorrentState::Paused).await;
}

#[tokio::test]
async fn pause_and_resume_are_idempotent() {
    let fixture = fixture().await;
    add_paused(&fixture).await.unwrap();
    wait_for_state(&fixture, TorrentState::Paused).await;

    fixture.engine.pause(fixture.id).await.unwrap();
    assert_eq!(state(&fixture), Some(TorrentState::Paused));

    fixture.engine.resume(fixture.id).await.unwrap();
    wait_for_state(&fixture, TorrentState::Running).await;
    fixture.engine.resume(fixture.id).await.unwrap();
    assert_eq!(state(&fixture), Some(TorrentState::Running));
}

#[tokio::test]
async fn pausing_or_resuming_an_unknown_torrent_is_rejected() {
    let fixture = fixture().await;

    let pause = fixture.engine.pause(fixture.id).await.unwrap_err();
    let resume = fixture.engine.resume(fixture.id).await.unwrap_err();

    assert!(matches!(pause, EngineError::UnknownTorrent(id) if id == fixture.id));
    assert!(matches!(resume, EngineError::UnknownTorrent(id) if id == fixture.id));
}
