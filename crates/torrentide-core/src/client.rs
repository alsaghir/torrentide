//! The actor that owns the engine; callers reach it only through a [`ClientHandle`].

use std::{
    collections::HashMap,
    io::{self, Write},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{self, Instant, MissedTickBehavior},
};

use crate::{
    AddOptions, AddSource, Database, DatabaseError, Engine, EngineError, RemoveMode, TorrentId,
    TorrentStatus, error_chain, reconcile,
};

/// Commands are rare; the bound only keeps a stalled actor from queueing without limit.
const COMMAND_CAPACITY: usize = 32;

/// Bounds the upload accounting a crash can lose to a minute between successful saves.
const SAVE_INTERVAL: Duration = Duration::from_secs(60);

/// While a handle exists the actor stops only by panicking, so this is the only failure.
const ACTOR_PANICKED: &str = "the client actor panicked";

type Reply<T> = oneshot::Sender<Result<T, ClientError>>;

/// Why a [`ClientHandle`] call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Database(#[from] DatabaseError),
}

#[derive(Debug)]
enum Command {
    Engine(Request),
    Shutdown,
}

/// A call forwarded to the engine, with the channel its result goes back on.
#[derive(Debug)]
enum Request {
    Add {
        source: AddSource,
        options: AddOptions,
        reply: Reply<TorrentId>,
    },
    Pause {
        id: TorrentId,
        reply: Reply<()>,
    },
    Resume {
        id: TorrentId,
        reply: Reply<()>,
    },
    Remove {
        id: TorrentId,
        mode: RemoveMode,
        reply: Reply<()>,
    },
}

/// A torrent's engine status and lifetime upload total as last published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Torrent {
    /// The latest status reported by the engine.
    pub status: TorrentStatus,
    /// Lifetime bytes uploaded, across pauses and restarts.
    pub uploaded: u64,
}

#[derive(Default)]
struct Counter {
    base: u64,
    last_session: u64,
    saved: u64,
}

impl Counter {
    fn total(&self) -> u64 {
        self.base.saturating_add(self.last_session)
    }
}

struct Uploads(HashMap<TorrentId, Counter>);

impl Uploads {
    fn load(database: &Database) -> Result<Self, DatabaseError> {
        let counters = database.torrents()?.into_iter().map(|record| {
            (
                record.id,
                Counter {
                    base: record.uploaded,
                    saved: record.uploaded,
                    last_session: 0,
                },
            )
        });
        Ok(Self(counters.collect()))
    }

    fn observe(&mut self, status: &TorrentStatus) -> u64 {
        let counter = self.0.entry(status.id).or_default();
        if status.session_uploaded < counter.last_session {
            counter.base = counter.total();
        }
        counter.last_session = status.session_uploaded;
        counter.total()
    }

    fn sample(&mut self, engine: &impl Engine) {
        for status in engine.snapshot() {
            self.observe(&status);
        }
    }

    fn snapshot(&mut self, engine: &impl Engine) -> Vec<Torrent> {
        engine
            .snapshot()
            .into_iter()
            .map(|status| Torrent {
                uploaded: self.observe(&status),
                status,
            })
            .collect()
    }

    fn save(&mut self, engine: &impl Engine, database: &mut Database) {
        self.sample(engine);
        let mut changed = self
            .0
            .iter()
            .filter_map(|(&id, counter)| {
                let total = counter.total();
                (total != counter.saved).then_some((id, total))
            })
            .peekable();
        if changed.peek().is_none() {
            return;
        }
        if let Err(error) = database.set_uploaded(changed) {
            // Keep the totals dirty for retry; a failed write must not stop the engine.
            // Even a closed stderr must not make reporting the failure fatal.
            let _ = writeln!(
                io::stderr().lock(),
                "torrentide: cannot save upload totals: {}",
                error_chain(&error)
            );
            return;
        }
        for counter in self.0.values_mut() {
            counter.saved = counter.total();
        }
    }
}

/// Runs an engine on its own task, keeps the database in step with it and publishes its
/// torrents.
///
/// Rows are written before an add and after a remove, so a failure or crash in between leaves
/// at most a row without a torrent, which nothing reads and the next reconcile deletes.
#[derive(Debug)]
pub struct ClientHandle<E> {
    commands: mpsc::Sender<Command>,
    snapshot: watch::Receiver<Vec<Torrent>>,
    actor: JoinHandle<E>,
}

impl<E: Engine> ClientHandle<E> {
    /// Reconciles the database with the engine, then starts the actor, which polls the engine
    /// every `tick` and publishes its torrents when they change.
    ///
    /// # Errors
    ///
    /// [`DatabaseError`] if reconciling or loading upload totals fails; nothing is started.
    ///
    /// # Panics
    ///
    /// If called outside a tokio runtime.
    pub fn spawn(engine: E, database: Database, tick: Duration) -> Result<Self, DatabaseError> {
        reconcile(&engine, &database, unix_now())?;
        let mut uploads = Uploads::load(&database)?;
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (publisher, snapshot) = watch::channel(uploads.snapshot(&engine));
        let actor = tokio::spawn(run(engine, database, tick, receiver, publisher, uploads));
        Ok(Self {
            commands,
            snapshot,
            actor,
        })
    }

    /// The engine's torrents as last published; `changed()` wakes only when they differ.
    #[must_use]
    pub fn snapshot(&self) -> watch::Receiver<Vec<Torrent>> {
        self.snapshot.clone()
    }

    /// Adds a torrent through [`Engine::add`] after giving it a row; on success the snapshot
    /// already includes it.
    ///
    /// # Errors
    ///
    /// [`ClientError::Database`] if the row cannot be written; the engine is not called.
    /// [`ClientError::Engine`] with the engine's error, unchanged.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn add(
        &self,
        source: AddSource,
        options: AddOptions,
    ) -> Result<TorrentId, ClientError> {
        self.request(|reply| Request::Add {
            source,
            options,
            reply,
        })
        .await
    }

    /// Pauses a torrent through [`Engine::pause`]; on success the snapshot already shows it.
    ///
    /// # Errors
    ///
    /// [`ClientError::Engine`] with the engine's error, unchanged.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn pause(&self, id: TorrentId) -> Result<(), ClientError> {
        self.request(|reply| Request::Pause { id, reply }).await
    }

    /// Resumes a torrent through [`Engine::resume`]; on success the snapshot already shows it.
    ///
    /// # Errors
    ///
    /// [`ClientError::Engine`] with the engine's error, unchanged.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn resume(&self, id: TorrentId) -> Result<(), ClientError> {
        self.request(|reply| Request::Resume { id, reply }).await
    }

    /// Removes a torrent through [`Engine::remove`], then its row; the snapshot no longer has
    /// it once the engine removed it.
    ///
    /// # Errors
    ///
    /// [`ClientError::Engine`] with the engine's error, unchanged; the row is kept.
    /// [`ClientError::Database`] if the row cannot be deleted. The torrent is removed already;
    /// the next reconcile deletes its row.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn remove(&self, id: TorrentId, mode: RemoveMode) -> Result<(), ClientError> {
        self.request(|reply| Request::Remove { id, mode, reply })
            .await
    }

    /// Stops the actor and returns the engine, so the caller can shut the engine down. The
    /// actor attempts to save outstanding upload totals before closing the database.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn shutdown(self) -> E {
        // Sending fails only if the actor has ended already; joining it reports why.
        let _ = self.commands.send(Command::Shutdown).await;
        self.actor.await.expect(ACTOR_PANICKED)
    }

    async fn request<T>(
        &self,
        request: impl FnOnce(Reply<T>) -> Request,
    ) -> Result<T, ClientError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Engine(request(reply)))
            .await
            .expect(ACTOR_PANICKED);
        result.await.expect(ACTOR_PANICKED)
    }
}

async fn run<E: Engine>(
    engine: E,
    mut database: Database,
    tick: Duration,
    mut commands: mpsc::Receiver<Command>,
    snapshot: watch::Sender<Vec<Torrent>>,
    mut uploads: Uploads,
) -> E {
    // `spawn` published the first snapshot, so the first poll is one tick away.
    let mut interval = time::interval_at(Instant::now() + tick, tick);
    // After a stall, poll once and carry on rather than catching up with a burst of polls.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut saves = time::interval_at(Instant::now() + SAVE_INTERVAL, SAVE_INTERVAL);
    saves.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Engine(request)) => {
                    execute(&engine, &mut database, &snapshot, &mut uploads, request).await;
                }
                Some(Command::Shutdown) | None => break,
            },
            _ = interval.tick() => publish_if_changed(&snapshot, uploads.snapshot(&engine)),
            _ = saves.tick() => uploads.save(&engine, &mut database),
        }
    }
    uploads.save(&engine, &mut database);
    engine
}

/// Engine calls run one at a time, so a slow call delays the polls queued behind it.
///
/// `database` is `&mut` because `Database` is not `Sync`: a shared reference held across an
/// `.await` would make the actor's future non-`Send`. The short SQLite operations run inline
/// instead of on a blocking thread.
async fn execute<E: Engine>(
    engine: &E,
    database: &mut Database,
    snapshot: &watch::Sender<Vec<Torrent>>,
    uploads: &mut Uploads,
    request: Request,
) {
    // The engine can discard uploads made since the last poll when a torrent stops.
    if matches!(request, Request::Pause { .. } | Request::Remove { .. }) {
        uploads.sample(engine);
    }
    match request {
        Request::Add {
            source,
            options,
            reply,
        } => respond(
            engine,
            snapshot,
            uploads,
            reply,
            add(engine, database, source, options).await,
        ),
        Request::Pause { id, reply } => {
            respond(
                engine,
                snapshot,
                uploads,
                reply,
                engine.pause(id).await.map_err(Into::into),
            );
        }
        Request::Resume { id, reply } => {
            respond(
                engine,
                snapshot,
                uploads,
                reply,
                engine.resume(id).await.map_err(Into::into),
            );
        }
        Request::Remove { id, mode, reply } => {
            let result = remove(engine, database, uploads, id, mode).await;
            respond(engine, snapshot, uploads, reply, result);
        }
    }
}

async fn add<E: Engine>(
    engine: &E,
    database: &mut Database,
    source: AddSource,
    options: AddOptions,
) -> Result<TorrentId, ClientError> {
    let id = engine.torrent_id(&source)?;
    let inserted = database.insert_torrent(id, unix_now())?;
    match engine.add(source, options).await {
        Ok(id) => Ok(id),
        Err(error) => {
            // On `AlreadyAdded` the row belongs to the torrent the engine has.
            if inserted && !matches!(error, EngineError::AlreadyAdded(_)) {
                // A row left behind is harmless; the next reconcile deletes it.
                let _ = database.delete_torrent(id);
            }
            Err(error.into())
        }
    }
}

async fn remove<E: Engine>(
    engine: &E,
    database: &mut Database,
    uploads: &mut Uploads,
    id: TorrentId,
    mode: RemoveMode,
) -> Result<(), ClientError> {
    engine.remove(id, mode).await?;
    uploads.0.remove(&id);
    // A missing row is fine: the row is gone either way.
    database.delete_torrent(id)?;
    Ok(())
}

/// Publishes before replying, so a caller sees its effect when the reply arrives. Failed calls
/// publish too: a remove whose row delete failed has still changed the engine.
fn respond<E: Engine, T>(
    engine: &E,
    snapshot: &watch::Sender<Vec<Torrent>>,
    uploads: &mut Uploads,
    reply: Reply<T>,
    result: Result<T, ClientError>,
) {
    publish_if_changed(snapshot, uploads.snapshot(engine));
    // The caller may have stopped waiting; the call is done either way.
    let _ = reply.send(result);
}

/// Unix time in seconds.
fn unix_now() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the system clock is after 1970");
    i64::try_from(since_epoch.as_secs()).expect("Unix seconds fit in i64")
}

/// Publishing an equal list would wake every receiver for nothing.
fn publish_if_changed(snapshot: &watch::Sender<Vec<Torrent>>, current: Vec<Torrent>) {
    snapshot.send_if_modified(|published| {
        let changed = *published != current;
        if changed {
            *published = current;
        }
        changed
    });
}
