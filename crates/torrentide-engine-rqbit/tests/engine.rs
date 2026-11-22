//! `RqbitEngine` against an isolated session: no network, the payload is already on disk.

mod common;

use std::path::Path;

use common::{PAYLOAD_NAME, isolated_session, payload_torrent};
use torrentide_core::{AddOptions, AddSource, Engine, EngineError, RemoveMode, TorrentId};
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
