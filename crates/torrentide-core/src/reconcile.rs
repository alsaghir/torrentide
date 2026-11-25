//! Brings the database rows in line with the torrents the engine restored.

use std::collections::HashSet;

use crate::{Database, DatabaseError, Engine, TorrentId};

/// What [`reconcile`] changed in the database.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Engine torrents that had no row, by info hash.
    pub inserted: Vec<TorrentId>,
    /// Rows whose torrent the engine no longer has.
    pub deleted: Vec<TorrentId>,
}

/// Gives every engine torrent exactly one row; the engine's list is the truth.
///
/// Run at startup, before any command: a crash between an engine add or remove and the
/// matching row change leaves the two lists apart. Missing rows are queued last, added at
/// `now` (Unix seconds), in info hash order so the result does not depend on the engine.
///
/// # Errors
///
/// [`DatabaseError`] if a row cannot be read or written. Changes made before the failure stay;
/// the next run finishes the rest.
pub fn reconcile(
    engine: &impl Engine,
    database: &Database,
    now: i64,
) -> Result<Reconciled, DatabaseError> {
    let in_engine: HashSet<TorrentId> = engine.snapshot().into_iter().map(|s| s.id).collect();
    let in_database: HashSet<TorrentId> = database
        .torrents()?
        .into_iter()
        .map(|record| record.id)
        .collect();

    let mut reconciled = Reconciled {
        inserted: in_engine.difference(&in_database).copied().collect(),
        deleted: in_database.difference(&in_engine).copied().collect(),
    };
    reconciled
        .inserted
        .sort_unstable_by_key(|id| id.info_hash());
    reconciled.deleted.sort_unstable_by_key(|id| id.info_hash());

    for &id in &reconciled.inserted {
        database.insert_torrent(id, now)?;
    }
    for &id in &reconciled.deleted {
        database.delete_torrent(id)?;
    }
    Ok(reconciled)
}
