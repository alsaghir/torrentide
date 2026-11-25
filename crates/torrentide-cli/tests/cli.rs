//! The `torrentide-cli` binary, run against a temporary portable root without network access.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;
use torrentide_core::{Database, Paths, Settings, SettingsFile, TorrentId};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_torrentide-cli"))
        .arg("--portable")
        .arg(root)
        .args(args)
        .output()
        .expect("run torrentide-cli")
}

fn stdout(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

/// A portable root whose settings keep the engine off the network.
fn root() -> TempDir {
    let root = tempfile::tempdir().expect("temp dir");
    let paths = Paths::portable(root.path());
    let mut settings = Settings::first_run(&paths);
    settings.downloads.dir = root.path().join("downloads");
    settings.bittorrent.dht = false;
    settings.bittorrent.lsd = false;
    fs::create_dir_all(&settings.downloads.dir).expect("downloads dir");
    SettingsFile::new(&paths)
        .save(&settings)
        .expect("settings saved");
    root
}

/// A one-byte, one-file torrent without trackers.
fn torrent_file(root: &Path) -> PathBuf {
    let mut torrent = b"d4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:".to_vec();
    torrent.extend([0; 20]);
    torrent.extend(b"ee");
    let path = root.join("a.torrent");
    fs::write(&path, torrent).expect("torrent written");
    path
}

fn add(root: &Path, file: &Path) -> Output {
    cli(
        root,
        &["add", "--paused", file.to_str().expect("UTF-8 path")],
    )
}

#[test]
fn list_on_a_fresh_root_prints_nothing() {
    let root = tempfile::tempdir().expect("temp dir");

    assert_eq!(stdout(cli(root.path(), &["list"])), "");
}

#[test]
fn list_prints_rows_in_queue_order() {
    let root = tempfile::tempdir().expect("temp dir");
    let database = Database::open(&Paths::portable(root.path())).expect("open database");
    assert!(
        database
            .insert_torrent(TorrentId::from_info_hash([0xbb; 20]), 200)
            .unwrap()
    );
    assert!(
        database
            .insert_torrent(TorrentId::from_info_hash([0xaa; 20]), 100)
            .unwrap()
    );
    drop(database);

    let expected = format!("0\t{}\t200\n1\t{}\t100\n", "bb".repeat(20), "aa".repeat(20));
    assert_eq!(stdout(cli(root.path(), &["list"])), expected);
}

#[test]
fn add_prints_the_id_and_list_shows_it() {
    let root = root();
    let file = torrent_file(root.path());

    let id = stdout(add(root.path(), &file));
    let listed = stdout(cli(root.path(), &["list"]));

    let id = id.trim_end();
    assert_eq!(id.len(), 40, "{id}");
    assert!(listed.starts_with(&format!("0\t{id}\t")), "{listed}");
    assert_eq!(listed.lines().count(), 1, "{listed}");
}

#[test]
fn adding_an_invalid_file_fails_and_adds_nothing() {
    let root = root();
    let file = root.path().join("broken.torrent");
    fs::write(&file, "not a torrent").expect("file written");

    let output = add(root.path(), &file);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 output");
    assert!(stderr.starts_with("torrentide-cli: "), "{stderr}");
    assert_eq!(stdout(cli(root.path(), &["list"])), "");
}

#[test]
fn adding_a_torrent_again_fails() {
    let root = root();
    let file = torrent_file(root.path());
    let id = stdout(add(root.path(), &file));

    let output = add(root.path(), &file);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 output");
    assert!(stderr.contains("already added"), "{stderr}");
    assert_eq!(stdout(cli(root.path(), &["list"])).lines().count(), 1);
    assert!(stderr.contains(id.trim_end()), "{stderr}");
}

#[test]
fn run_prints_the_restored_torrents() {
    let root = root();
    let file = torrent_file(root.path());
    let id = stdout(add(root.path(), &file));
    let expected = format!("{}\tpaused", id.trim_end());
    let mut run = Command::new(env!("CARGO_BIN_EXE_torrentide-cli"))
        .arg("--portable")
        .arg(root.path())
        .arg("run")
        .stdout(Stdio::piped())
        .spawn()
        .expect("run torrentide-cli");
    let mut lines = BufReader::new(run.stdout.take().expect("piped stdout")).lines();

    // The torrent may show as checking before it settles as paused.
    let mut seen = Vec::new();
    loop {
        let snapshot: Vec<String> = lines
            .by_ref()
            .map(|line| line.expect("UTF-8 line"))
            .take_while(|line| !line.is_empty())
            .collect();
        if snapshot == [expected.as_str()] {
            break;
        }
        seen.push(snapshot);
        assert!(seen.len() < 10, "no `{expected}` in {seen:?}");
    }

    run.kill().expect("run killed");
    run.wait().expect("run reaped");
}
