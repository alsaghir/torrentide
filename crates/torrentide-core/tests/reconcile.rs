//! Startup reconcile between an engine's torrents and the database rows.

use torrentide_core::{
    AddOptions, AddSource, Database, Engine, EngineError, Paths, Reconciled, RemoveMode, TorrentId,
    TorrentRecord, TorrentState, TorrentStatus, reconcile,
};

/// Panics when called: [`reconcile`] must not change the engine.
fn unused<T>() -> std::future::Ready<T> {
    unreachable!("reconcile only reads the snapshot")
}

/// An engine that only reports a fixed list of torrents.
struct FixedEngine(Vec<TorrentId>);

impl Engine for FixedEngine {
    fn torrent_id(&self, _: &AddSource) -> Result<TorrentId, EngineError> {
        unreachable!("reconcile only reads the snapshot")
    }

    fn add(
        &self,
        _: AddSource,
        _: AddOptions,
    ) -> impl Future<Output = Result<TorrentId, EngineError>> {
        unused()
    }

    fn pause(&self, _: TorrentId) -> impl Future<Output = Result<(), EngineError>> {
        unused()
    }

    fn resume(&self, _: TorrentId) -> impl Future<Output = Result<(), EngineError>> {
        unused()
    }

    fn remove(&self, _: TorrentId, _: RemoveMode) -> impl Future<Output = Result<(), EngineError>> {
        unused()
    }

    fn snapshot(&self) -> Vec<TorrentStatus> {
        self.0
            .iter()
            .map(|&id| TorrentStatus {
                id,
                state: TorrentState::Paused,
                name: None,
                size: 0,
                done: 0,
                download_rate: 0,
                upload_rate: 0,
            })
            .collect()
    }
}

fn id(byte: u8) -> TorrentId {
    TorrentId::from_info_hash([byte; 20])
}

fn database() -> (tempfile::TempDir, Database) {
    let root = tempfile::tempdir().expect("temp dir");
    let database = Database::open(&Paths::portable(root.path())).unwrap();
    (root, database)
}

fn ids(database: &Database) -> Vec<TorrentId> {
    database
        .torrents()
        .unwrap()
        .into_iter()
        .map(|record| record.id)
        .collect()
}

#[test]
fn matching_lists_are_left_alone() {
    let (_root, database) = database();
    database.insert_torrent(id(1), 100).unwrap();
    let before = database.torrents().unwrap();

    let reconciled = reconcile(&FixedEngine(vec![id(1)]), &database, 500).unwrap();

    assert_eq!(reconciled, Reconciled::default());
    assert_eq!(database.torrents().unwrap(), before);
}

#[test]
fn engine_torrents_without_a_row_are_queued_last() {
    let (_root, database) = database();
    database.insert_torrent(id(1), 100).unwrap();

    let reconciled = reconcile(&FixedEngine(vec![id(3), id(1), id(2)]), &database, 500).unwrap();

    assert_eq!(
        reconciled,
        Reconciled {
            inserted: vec![id(2), id(3)],
            deleted: vec![],
        }
    );
    assert_eq!(
        database.torrents().unwrap()[1..],
        [
            TorrentRecord {
                id: id(2),
                queue_position: 1,
                added_at: 500,
                completed_at: None,
            },
            TorrentRecord {
                id: id(3),
                queue_position: 2,
                added_at: 500,
                completed_at: None,
            },
        ]
    );
}

#[test]
fn rows_without_an_engine_torrent_are_deleted() {
    let (_root, database) = database();
    database.insert_torrent(id(1), 100).unwrap();
    database.insert_torrent(id(2), 200).unwrap();

    let reconciled = reconcile(&FixedEngine(vec![id(2)]), &database, 500).unwrap();

    assert_eq!(
        reconciled,
        Reconciled {
            inserted: vec![],
            deleted: vec![id(1)],
        }
    );
    assert_eq!(ids(&database), [id(2)]);
}

#[test]
fn reconciling_twice_changes_nothing_the_second_time() {
    let (_root, database) = database();
    database.insert_torrent(id(1), 100).unwrap();
    let engine = FixedEngine(vec![id(2)]);
    reconcile(&engine, &database, 500).unwrap();

    let again = reconcile(&engine, &database, 600).unwrap();

    assert_eq!(again, Reconciled::default());
    assert_eq!(ids(&database), [id(2)]);
}
