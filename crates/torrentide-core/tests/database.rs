//! The database file on disk, under a temporary portable root.

use std::fs;

use rusqlite::Connection;
use torrentide_core::{Database, DatabaseError, Paths};

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
    assert_eq!(user_version(&connection), 1);
    let torrents: i64 = connection
        .query_row("SELECT count(*) FROM torrent", [], |row| row.get(0))
        .unwrap();
    assert_eq!(torrents, 0);
}

#[test]
fn reopening_keeps_the_rows() {
    let (_root, paths) = portable();
    drop(Database::open(&paths).unwrap());
    Connection::open(Database::file_path(&paths))
        .unwrap()
        .execute(
            "INSERT INTO torrent (info_hash, queue_position, added_at) VALUES (?1, 0, 0)",
            [[0xAB_u8; 20]],
        )
        .unwrap();

    drop(Database::open(&paths).unwrap());

    let connection = Connection::open(Database::file_path(&paths)).unwrap();
    let torrents: i64 = connection
        .query_row("SELECT count(*) FROM torrent", [], |row| row.get(0))
        .unwrap();
    assert_eq!(torrents, 1);
    assert_eq!(user_version(&connection), 1);
}

#[test]
fn a_newer_schema_is_refused() {
    let (_root, paths) = portable();
    drop(Database::open(&paths).unwrap());
    Connection::open(Database::file_path(&paths))
        .unwrap()
        .pragma_update(None, "user_version", 2)
        .unwrap();

    let error = Database::open(&paths).unwrap_err();

    assert!(
        matches!(
            error,
            DatabaseError::TooNew {
                found: 2,
                supported: 1,
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
