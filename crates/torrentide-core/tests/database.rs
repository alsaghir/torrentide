//! The database file on disk, under a temporary portable root.

use std::fs;

use rusqlite::Connection;
use torrentide_core::{Database, DatabaseError, Paths, TorrentId, TorrentRecord};

fn portable() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().expect("temp dir");
    let paths = Paths::portable(root.path());
    (root, paths)
}

fn user_version(connection: &Connection) -> i64 {
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[test]
fn opening_creates_the_current_schema() {
    let (_root, paths) = portable();

    drop(Database::open(&paths).unwrap());

    let connection = Connection::open(Database::file_path(&paths)).unwrap();
    assert_eq!(user_version(&connection), 2);
    let torrents: i64 = connection
        .query_row("SELECT count(*) FROM torrent", [], |row| row.get(0))
        .unwrap();
    assert_eq!(torrents, 0);
}

#[test]
fn migrating_from_version_one_keeps_rows_with_zero_uploads() {
    let (_root, paths) = portable();
    fs::create_dir_all(paths.data_dir()).unwrap();
    let connection = Connection::open(Database::file_path(&paths)).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE torrent (
                id INTEGER PRIMARY KEY,
                info_hash BLOB NOT NULL UNIQUE,
                queue_position INTEGER NOT NULL,
                added_at INTEGER NOT NULL,
                completed_at INTEGER
            ) STRICT;
            PRAGMA user_version = 1;",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO torrent (info_hash, queue_position, added_at, completed_at) \
             VALUES (?1, 3, 100, 200)",
            [id(1).info_hash()],
        )
        .unwrap();
    drop(connection);

    let db = Database::open(&paths).unwrap();

    assert_eq!(
        db.torrents().unwrap(),
        [TorrentRecord {
            id: id(1),
            queue_position: 3,
            added_at: 100,
            completed_at: Some(200),
            uploaded: 0,
        }]
    );
}

#[test]
fn a_newer_schema_is_refused() {
    let (_root, paths) = portable();
    drop(Database::open(&paths).unwrap());
    Connection::open(Database::file_path(&paths))
        .unwrap()
        .pragma_update(None, "user_version", 3)
        .unwrap();

    let error = Database::open(&paths).unwrap_err();

    assert!(
        matches!(
            error,
            DatabaseError::TooNew {
                found: 3,
                supported: 2,
                ..
            }
        ),
        "{error:?}"
    );
}

#[test]
fn a_file_that_is_not_a_database_is_refused() {
    let (_root, paths) = portable();
    fs::create_dir_all(paths.data_dir()).unwrap();
    fs::write(Database::file_path(&paths), [0xAB_u8; 4096]).unwrap();

    let error = Database::open(&paths).unwrap_err();

    assert!(matches!(error, DatabaseError::Sqlite { .. }), "{error:?}");
    assert_eq!(
        fs::read(Database::file_path(&paths)).unwrap(),
        [0xAB_u8; 4096],
        "the file is left untouched"
    );
}

fn id(byte: u8) -> TorrentId {
    TorrentId::from_info_hash([byte; 20])
}

#[test]
fn added_torrents_are_listed_in_queue_order() {
    let (_root, paths) = portable();
    let db = Database::open(&paths).unwrap();

    assert!(db.insert_torrent(id(1), 100).unwrap());
    assert!(db.insert_torrent(id(2), 200).unwrap());

    assert_eq!(
        db.torrents().unwrap(),
        vec![
            TorrentRecord {
                id: id(1),
                queue_position: 0,
                added_at: 100,
                completed_at: None,
                uploaded: 0,
            },
            TorrentRecord {
                id: id(2),
                queue_position: 1,
                added_at: 200,
                completed_at: None,
                uploaded: 0,
            },
        ]
    );
}

#[test]
fn adding_a_known_torrent_keeps_its_row() {
    let (_root, paths) = portable();
    let db = Database::open(&paths).unwrap();
    db.insert_torrent(id(1), 100).unwrap();

    assert!(!db.insert_torrent(id(1), 200).unwrap());
    assert_eq!(
        db.torrents().unwrap(),
        [TorrentRecord {
            id: id(1),
            queue_position: 0,
            added_at: 100,
            completed_at: None,
            uploaded: 0,
        }]
    );
}

#[test]
fn removing_reports_whether_the_torrent_was_known() {
    let (_root, paths) = portable();
    let db = Database::open(&paths).unwrap();
    db.insert_torrent(id(1), 100).unwrap();

    assert!(db.delete_torrent(id(1)).unwrap());
    assert!(!db.delete_torrent(id(1)).unwrap());
    assert_eq!(db.torrents().unwrap(), []);
}

#[test]
fn removing_keeps_the_other_positions() {
    let (_root, paths) = portable();
    let db = Database::open(&paths).unwrap();
    db.insert_torrent(id(1), 100).unwrap();
    db.insert_torrent(id(2), 200).unwrap();
    db.insert_torrent(id(3), 300).unwrap();

    db.delete_torrent(id(2)).unwrap();

    let rows = db.torrents().unwrap();
    let positions: Vec<_> = rows.iter().map(|r| r.queue_position).collect();
    let ids: Vec<_> = rows.iter().map(|r| r.id).collect();
    assert_eq!(positions, [0, 2]);
    assert_eq!(ids, [id(1), id(3)]);
}

#[test]
fn torrent_rows_survive_reopening() {
    let (_root, paths) = portable();
    let db = Database::open(&paths).unwrap();
    db.insert_torrent(id(1), 100).unwrap();
    drop(db);

    let db = Database::open(&paths).unwrap();

    assert_eq!(db.torrents().unwrap().len(), 1);
    assert_eq!(db.torrents().unwrap()[0].id, id(1));
}

#[test]
fn upload_totals_survive_reopening() {
    let (_root, paths) = portable();
    let mut db = Database::open(&paths).unwrap();
    db.insert_torrent(id(1), 100).unwrap();
    db.insert_torrent(id(2), 200).unwrap();
    let largest = u64::try_from(i64::MAX).unwrap();
    db.set_uploaded([(id(1), 1000), (id(2), largest), (id(3), 10)])
        .unwrap();
    drop(db);

    let db = Database::open(&paths).unwrap();
    let totals: Vec<_> = db
        .torrents()
        .unwrap()
        .into_iter()
        .map(|record| (record.id, record.uploaded))
        .collect();
    assert_eq!(totals, [(id(1), 1000), (id(2), largest)]);
}

#[test]
fn an_out_of_range_upload_total_leaves_the_batch_unwritten() {
    let (_root, paths) = portable();
    let mut db = Database::open(&paths).unwrap();
    db.insert_torrent(id(1), 100).unwrap();
    db.insert_torrent(id(2), 200).unwrap();

    let error = db
        .set_uploaded([(id(1), 1000), (id(2), u64::MAX)])
        .unwrap_err();

    assert!(matches!(error, DatabaseError::Sqlite { .. }), "{error:?}");
    let rows = db.torrents().unwrap();
    assert_eq!((rows[0].uploaded, rows[1].uploaded), (0, 0));
}
