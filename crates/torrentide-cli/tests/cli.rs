//! The `torrentide-cli` binary, run against a temporary portable root.

use std::path::Path;
use std::process::{Command, Output};

use torrentide_core::{Database, Paths, TorrentId};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_torrentide-cli"))
        .arg("--portable")
        .arg(root)
        .args(args)
        .output()
        .expect("run torrentide-cli")
}

#[test]
fn list_on_a_fresh_root_prints_nothing() {
    let root = tempfile::tempdir().expect("temp dir");

    let output = cli(root.path(), &["list"]);

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
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

    let output = cli(root.path(), &["list"]);

    assert!(output.status.success(), "{output:?}");
    let expected = format!("0\t{}\t200\n1\t{}\t100\n", "bb".repeat(20), "aa".repeat(20));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}
