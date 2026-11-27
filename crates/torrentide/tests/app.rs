//! Exercises the desktop composition against a headless window and portable settings.

use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
use std::num::NonZeroU16;
use std::time::Duration;

use tempfile::TempDir;
use tokio::runtime::Runtime;
use tokio::time;
use torrentide::App;
use torrentide_core::{
    AddOptions, AddSource, Paths, Settings, SettingsFile, Torrent, TorrentState,
};
use torrentide_gui::columns::{self, ColumnId};
use torrentide_testkit::{PAYLOAD_NAME, payload_torrent, seed};

const SNAPSHOT_DEADLINE: Duration = Duration::from_secs(10);
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(60);

fn portable_root(port: NonZeroU16) -> (TempDir, Paths) {
    let root = tempfile::tempdir().expect("portable root");
    let paths = Paths::portable(root.path());
    let mut settings = Settings::first_run(&paths);
    settings.downloads.dir = root.path().join("downloads");
    settings.bittorrent.dht = false;
    settings.bittorrent.lsd = false;
    settings.connection.listen_port = port;
    fs::create_dir_all(&settings.downloads.dir).expect("downloads dir");
    SettingsFile::new(&paths)
        .save(&settings)
        .expect("settings saved");
    (root, paths)
}

fn free_port() -> NonZeroU16 {
    let listener = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)).expect("bind a free port");
    let port = listener.local_addr().expect("bound address").port();
    NonZeroU16::new(port).expect("the OS never assigns port 0")
}

fn one_byte_torrent() -> AddSource {
    let mut torrent = b"d4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:".to_vec();
    torrent.extend([0; 20]);
    torrent.extend(b"ee");
    AddSource::TorrentFile(torrent)
}

fn column(id: ColumnId) -> usize {
    columns::builtin()
        .iter()
        .filter(|column| column.default_visible)
        .position(|column| column.id == id)
        .expect("column is visible")
}

fn run_until(
    app: App,
    runtime: &Runtime,
    deadline: Duration,
    mut ready: impl FnMut(&[Torrent]) -> bool + Send + 'static,
) -> Vec<Vec<String>> {
    let closer = app.window().closer();
    let mut snapshots = app.client().snapshot();
    let waiter = runtime.spawn(async move {
        let snapshot = time::timeout(deadline, snapshots.wait_for(|torrents| ready(torrents)))
            .await
            .map(|result| result.map(drop));
        (snapshot, closer.close())
    });

    let run = app.run();
    let rows = app.window().cell_texts();
    app.shutdown();

    run.expect("window ran");
    let (snapshot, close) = runtime
        .block_on(waiter)
        .expect("snapshot task did not panic");
    close.expect("close request was queued");
    snapshot
        .expect("timed out waiting for a matching torrent snapshot")
        .expect("snapshot channel closed");
    rows
}

#[test]
fn stored_torrent_is_shown_and_restored_after_closing() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let (root, paths) = portable_root(free_port());
    let runtime = Runtime::new().expect("test runtime");
    let app = App::start(&paths).expect("start desktop app");
    let id = runtime
        .block_on(app.client().add(
            one_byte_torrent(),
            AddOptions {
                save_path: root.path().join("downloads"),
                start_paused: true,
            },
        ))
        .expect("add paused torrent");
    let rows = run_until(app, &runtime, SNAPSHOT_DEADLINE, move |torrents| {
        torrents.iter().any(|torrent| torrent.status.id == id)
    });
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][column(ColumnId::NAME)], "a");

    let app = App::start(&paths).expect("restart desktop app");
    let rows = run_until(app, &runtime, SNAPSHOT_DEADLINE, |_| true);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][column(ColumnId::NAME)], "a");
}

#[test]
fn download_from_a_peer_reaches_seeding() {
    i_slint_backend_testing::init_integration_test_with_system_time();
    let port = free_port();
    let (root, paths) = portable_root(port);
    let runtime = Runtime::new().expect("seeder runtime");
    let seed_dir = tempfile::tempdir().expect("seeder dir");
    let (content, torrent) = runtime.block_on(payload_torrent(seed_dir.path()));
    let app = App::start(&paths).expect("start desktop app");
    let id = runtime
        .block_on(app.client().add(
            AddSource::TorrentFile(torrent.as_bytes().expect("torrent serialises").to_vec()),
            AddOptions {
                save_path: root.path().join("downloads"),
                start_paused: false,
            },
        ))
        .expect("add running torrent");
    let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, port.get()));
    let _seeder = runtime.block_on(seed(seed_dir.path(), &torrent, peer));

    let rows = run_until(app, &runtime, DOWNLOAD_DEADLINE, move |torrents| {
        torrents.iter().any(|torrent| {
            let status = &torrent.status;
            status.id == id
                && status.state == TorrentState::Running
                && status.size > 0
                && status.done >= status.size
        })
    });
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][column(ColumnId::NAME)], PAYLOAD_NAME);
    assert_eq!(rows[0][column(ColumnId::PROGRESS)], "100.0%");
    assert_eq!(rows[0][column(ColumnId::STATE)], "Seeding");
    assert_eq!(
        fs::read(root.path().join("downloads").join(PAYLOAD_NAME))
            .expect("read downloaded payload"),
        content
    );
}
