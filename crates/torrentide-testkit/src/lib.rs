//! Isolated librqbit sessions and a generated payload for integration and end-to-end tests.

use std::{path::Path, sync::Arc};

use librqbit::{
    CreateTorrentOptions, CreateTorrentResult, ListenerOptions, Session, SessionOptions,
    create_torrent, spawn_utils::BlockingSpawner,
};

const PIECE_LENGTH: u32 = 32 * 1024;
// Not a multiple of the piece length, so the short last piece is covered too.
const PAYLOAD_LENGTH: usize = 10 * PIECE_LENGTH as usize + 123;
pub const PAYLOAD_NAME: &str = "payload.bin";

/// A session with no DHT, LSD or listener unless given: it reaches only peers it is told about.
///
/// # Panics
///
/// If the session cannot start.
pub async fn isolated_session(dir: &Path, listen: Option<ListenerOptions>) -> Arc<Session> {
    Session::new_with_opts(
        dir.to_owned(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            listen,
            ..SessionOptions::default()
        },
    )
    .await
    .expect("session starts")
}

/// Writes [`PAYLOAD_NAME`] into `dir` and returns its content and a torrent for it.
///
/// # Panics
///
/// If the file cannot be written or the torrent cannot be created.
pub async fn payload_torrent(dir: &Path) -> (Vec<u8>, CreateTorrentResult) {
    let content: Vec<u8> = (0..PAYLOAD_LENGTH)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    let file = dir.join(PAYLOAD_NAME);
    std::fs::write(&file, &content).unwrap();
    let torrent = create_torrent(
        &file,
        CreateTorrentOptions {
            piece_length: Some(PIECE_LENGTH),
            ..CreateTorrentOptions::default()
        },
        &BlockingSpawner::new(1),
    )
    .await
    .unwrap();
    (content, torrent)
}
