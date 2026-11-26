//! Downloads a torrent from an in-process seeder over loopback: no DHT, tracker or LSD.

use std::{net::Ipv4Addr, path::Path, time::Duration};

use librqbit::{AddTorrent, AddTorrentOptions, ListenerOptions};
use torrentide_engine_rqbit::download;
use torrentide_testkit::{PAYLOAD_NAME, isolated_session, payload_torrent};

fn folder(path: &Path) -> String {
    path.to_str().expect("temp paths are UTF-8").to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn downloads_file_from_local_seeder() {
    let seed_dir = tempfile::tempdir().unwrap();
    let (content, torrent) = payload_torrent(seed_dir.path()).await;
    let torrent_bytes = torrent.as_bytes().unwrap();

    let seeder = isolated_session(
        seed_dir.path(),
        Some(ListenerOptions {
            listen_addr: (Ipv4Addr::LOCALHOST, 0).into(),
            ..ListenerOptions::default()
        }),
    )
    .await;
    // The data is already on disk, so this returns once the hash check passes.
    download(
        &seeder,
        AddTorrent::from_bytes(torrent_bytes.clone()),
        AddTorrentOptions {
            output_folder: Some(folder(&torrent.output_folder)),
            overwrite: true,
            ..AddTorrentOptions::default()
        },
    )
    .await
    .unwrap();
    let seeder_addr = seeder.listen_addr().expect("seeder listens");

    let leech_dir = tempfile::tempdir().unwrap();
    let leecher = isolated_session(leech_dir.path(), None).await;
    tokio::time::timeout(
        Duration::from_secs(60),
        download(
            &leecher,
            AddTorrent::from_bytes(torrent_bytes),
            AddTorrentOptions {
                output_folder: Some(folder(leech_dir.path())),
                initial_peers: Some(vec![seeder_addr]),
                ..AddTorrentOptions::default()
            },
        ),
    )
    .await
    .expect("download finishes within 60 s")
    .unwrap();

    assert_eq!(
        std::fs::read(leech_dir.path().join(PAYLOAD_NAME)).unwrap(),
        content
    );
}
