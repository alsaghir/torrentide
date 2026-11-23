//! Torrentide's own database: facts the engine does not keep, keyed by info hash.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use rusqlite::Connection;

use crate::Paths;

const FILE_NAME: &str = "torrentide.sqlite3";

/// Schema migrations in order; applying migration `i` sets `PRAGMA user_version` to `i + 1`.
/// Released migrations are never edited, only appended to.
const MIGRATIONS: &[&str] = &["
    CREATE TABLE torrent (
      id             INTEGER PRIMARY KEY,
      info_hash      BLOB NOT NULL UNIQUE,
      queue_position INTEGER NOT NULL,
      added_at       INTEGER NOT NULL,
      completed_at   INTEGER
    ) STRICT;
"];

/// An open database with the current schema.
#[derive(Debug)]
pub struct Database {
    #[expect(dead_code, reason = "read by the torrent queries of the next unit")]
    connection: Connection,
}

/// The database cannot be opened or brought to the current schema.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("cannot create {}", .path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("database {} cannot be used", .path.display())]
    Sqlite {
        path: PathBuf,
        #[source]
        source: rusqlite::Error,
    },
    #[error(
        "database {} has schema version {found}, newer than the supported {supported}",
        .path.display()
    )]
    TooNew {
        path: PathBuf,
        found: u32,
        supported: usize,
    },
}

impl Database {
    /// Where [`Database::open`] keeps the database.
    #[must_use]
    pub fn file_path(paths: &Paths) -> PathBuf {
        paths.data_dir().join(FILE_NAME)
    }

    /// Opens or creates the database and applies pending migrations.
    ///
    /// # Errors
    ///
    /// [`DatabaseError`] when the file is not a usable SQLite database or was written by a newer
    /// Torrentide; the client must not start then, so no data is overwritten.
    pub fn open(paths: &Paths) -> Result<Self, DatabaseError> {
        let dir = paths.data_dir();
        fs::create_dir_all(dir).map_err(|source| DatabaseError::CreateDir {
            path: dir.to_owned(),
            source,
        })?;
        let path = Self::file_path(paths);
        let sqlite = |source| DatabaseError::Sqlite {
            path: path.clone(),
            source,
        };
        let mut connection = Connection::open(&path).map_err(sqlite)?;
        // Readers do not block the writer; the pragma returns the new mode, hence `_and_check`.
        connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
            .map_err(sqlite)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(sqlite)?;
        migrate(&mut connection, &path)?;
        Ok(Self { connection })
    }
}

fn migrate(connection: &mut Connection, path: &Path) -> Result<(), DatabaseError> {
    let sqlite = |source| DatabaseError::Sqlite {
        path: path.to_owned(),
        source,
    };
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(sqlite)?;
    let pending = usize::try_from(version)
        .ok()
        .and_then(|version| MIGRATIONS.get(version..))
        .ok_or(DatabaseError::TooNew {
            path: path.to_owned(),
            found: version,
            supported: MIGRATIONS.len(),
        })?;
    for (applied, sql) in (version + 1..).zip(pending) {
        // One transaction per migration: a failure leaves the previous version intact.
        let transaction = connection.transaction().map_err(sqlite)?;
        transaction.execute_batch(sql).map_err(sqlite)?;
        transaction
            .pragma_update(None, "user_version", applied)
            .map_err(sqlite)?;
        transaction.commit().map_err(sqlite)?;
    }
    Ok(())
}
