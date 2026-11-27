//! Torrentide's own database: facts the engine does not keep, keyed by info hash.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use rusqlite::Connection;

use crate::{Paths, TorrentId};

const FILE_NAME: &str = "torrentide.sqlite3";

/// Schema migrations in order; applying migration `i` sets `PRAGMA user_version` to `i + 1`.
/// Released migrations are never edited, only appended to.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE torrent (
      id             INTEGER PRIMARY KEY,
      info_hash      BLOB NOT NULL UNIQUE,
      queue_position INTEGER NOT NULL,
      added_at       INTEGER NOT NULL,
      completed_at   INTEGER
    ) STRICT;
",
    "ALTER TABLE torrent ADD COLUMN uploaded INTEGER NOT NULL DEFAULT 0;",
];

/// Torrentide's data about one torrent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorrentRecord {
    pub id: TorrentId,
    /// Lower runs first; positions can have gaps after removals.
    pub queue_position: u32,
    /// Unix time in seconds.
    pub added_at: i64,
    /// Unix time in seconds; `None` until the download completed.
    pub completed_at: Option<i64>,
    /// Lifetime bytes uploaded as last saved.
    pub uploaded: u64,
}

/// An open database with the current schema.
#[derive(Debug)]
pub struct Database {
    connection: Connection,
    path: PathBuf,
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
        Ok(Self { connection, path })
    }

    fn sqlite(&self) -> impl Fn(rusqlite::Error) -> DatabaseError + '_ {
        |source| DatabaseError::Sqlite {
            path: self.path.clone(),
            source,
        }
    }

    /// Appends a torrent to the end of the queue. Returns `false`, changing nothing, if the
    /// torrent is already stored.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Sqlite`] if the database cannot be written.
    pub fn insert_torrent(&self, id: TorrentId, added_at: i64) -> Result<bool, DatabaseError> {
        let changed = self
            .connection
            .execute(
                // `WHERE true` lets SQLite parse `ON CONFLICT` after a `SELECT`.
                "INSERT INTO torrent (info_hash, queue_position, added_at) \
                 SELECT ?1, coalesce(max(queue_position) + 1, 0), ?2 FROM torrent WHERE true \
                 ON CONFLICT (info_hash) DO NOTHING",
                rusqlite::params![id.info_hash(), added_at],
            )
            .map_err(self.sqlite())?;
        Ok(changed > 0)
    }

    /// Removes a torrent row. Returns `true` if a row was deleted.
    ///
    /// Other positions are not renumbered.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Sqlite`] if the database cannot be written.
    pub fn delete_torrent(&self, id: TorrentId) -> Result<bool, DatabaseError> {
        let changed = self
            .connection
            .execute("DELETE FROM torrent WHERE info_hash = ?1", [id.info_hash()])
            .map_err(self.sqlite())?;
        Ok(changed > 0)
    }

    /// Saves lifetime upload totals in one transaction, ignoring missing rows.
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Sqlite`] if a total exceeds SQLite's signed integer range or the
    /// database cannot be written. No updates are committed on failure.
    pub fn set_uploaded(
        &mut self,
        totals: impl IntoIterator<Item = (TorrentId, u64)>,
    ) -> Result<(), DatabaseError> {
        let path = &self.path;
        let sqlite = |source| DatabaseError::Sqlite {
            path: path.clone(),
            source,
        };
        let transaction = self.connection.transaction().map_err(sqlite)?;
        {
            let mut statement = transaction
                .prepare("UPDATE torrent SET uploaded = ?1 WHERE info_hash = ?2")
                .map_err(sqlite)?;
            for (id, uploaded) in totals {
                statement
                    .execute(rusqlite::params![uploaded, id.info_hash()])
                    .map_err(sqlite)?;
            }
        }
        transaction.commit().map_err(sqlite)
    }

    /// All torrent rows, ordered by queue position (ascending).
    ///
    /// # Errors
    ///
    /// [`DatabaseError::Sqlite`] if the database cannot be read.
    pub fn torrents(&self) -> Result<Vec<TorrentRecord>, DatabaseError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT info_hash, queue_position, added_at, completed_at, uploaded \
                 FROM torrent ORDER BY queue_position",
            )
            .map_err(self.sqlite())?;
        let rows = statement
            .query_map([], |row| {
                let info_hash: [u8; 20] = row.get(0)?;
                Ok(TorrentRecord {
                    id: TorrentId::from_info_hash(info_hash),
                    queue_position: row.get(1)?,
                    added_at: row.get(2)?,
                    completed_at: row.get(3)?,
                    uploaded: row.get(4)?,
                })
            })
            .map_err(self.sqlite())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(self.sqlite())
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
