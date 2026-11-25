//! The actor that owns the engine; callers reach it only through a [`ClientHandle`].

use std::time::Duration;

use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{self, Instant, MissedTickBehavior},
};

use crate::{AddOptions, AddSource, Engine, EngineError, RemoveMode, TorrentId, TorrentStatus};

/// Commands are rare; the bound only keeps a stalled actor from queueing without limit.
const COMMAND_CAPACITY: usize = 32;

/// While a handle exists the actor stops only by panicking, so this is the only failure.
const ACTOR_PANICKED: &str = "the client actor panicked";

type Reply<T> = oneshot::Sender<Result<T, EngineError>>;

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

/// Runs an engine on its own task and publishes its torrents.
#[derive(Debug)]
pub struct ClientHandle<E> {
    commands: mpsc::Sender<Command>,
    snapshot: watch::Receiver<Vec<TorrentStatus>>,
    actor: JoinHandle<E>,
}

impl<E: Engine> ClientHandle<E> {
    /// Starts the actor, which polls the engine every `tick` and publishes its torrents when
    /// they change.
    ///
    /// # Panics
    ///
    /// If called outside a tokio runtime.
    #[must_use]
    pub fn spawn(engine: E, tick: Duration) -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (publisher, snapshot) = watch::channel(engine.snapshot());
        let actor = tokio::spawn(run(engine, tick, receiver, publisher));
        Self {
            commands,
            snapshot,
            actor,
        }
    }

    /// The engine's torrents as last published; `changed()` wakes only when they differ.
    #[must_use]
    pub fn snapshot(&self) -> watch::Receiver<Vec<TorrentStatus>> {
        self.snapshot.clone()
    }

    /// Adds a torrent through [`Engine::add`]; on success the snapshot already includes it.
    ///
    /// # Errors
    ///
    /// The engine's error, unchanged.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn add(
        &self,
        source: AddSource,
        options: AddOptions,
    ) -> Result<TorrentId, EngineError> {
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
    /// The engine's error, unchanged.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn pause(&self, id: TorrentId) -> Result<(), EngineError> {
        self.request(|reply| Request::Pause { id, reply }).await
    }

    /// Resumes a torrent through [`Engine::resume`]; on success the snapshot already shows it.
    ///
    /// # Errors
    ///
    /// The engine's error, unchanged.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn resume(&self, id: TorrentId) -> Result<(), EngineError> {
        self.request(|reply| Request::Resume { id, reply }).await
    }

    /// Removes a torrent through [`Engine::remove`]; on success the snapshot no longer has it.
    ///
    /// # Errors
    ///
    /// The engine's error, unchanged.
    ///
    /// # Panics
    ///
    /// If the actor panicked.
    pub async fn remove(&self, id: TorrentId, mode: RemoveMode) -> Result<(), EngineError> {
        self.request(|reply| Request::Remove { id, mode, reply })
            .await
    }

    /// Stops the actor and returns the engine, so the caller can shut the engine down.
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
    ) -> Result<T, EngineError> {
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
    tick: Duration,
    mut commands: mpsc::Receiver<Command>,
    snapshot: watch::Sender<Vec<TorrentStatus>>,
) -> E {
    // `spawn` published the first snapshot, so the first poll is one tick away.
    let mut interval = time::interval_at(Instant::now() + tick, tick);
    // After a stall, poll once and carry on rather than catching up with a burst of polls.
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Engine(request)) => execute(&engine, &snapshot, request).await,
                Some(Command::Shutdown) | None => break,
            },
            _ = interval.tick() => publish_if_changed(&snapshot, engine.snapshot()),
        }
    }
    engine
}

/// Engine calls run one at a time, so a slow call delays the polls queued behind it.
async fn execute<E: Engine>(
    engine: &E,
    snapshot: &watch::Sender<Vec<TorrentStatus>>,
    request: Request,
) {
    match request {
        Request::Add {
            source,
            options,
            reply,
        } => respond(engine, snapshot, reply, engine.add(source, options).await),
        Request::Pause { id, reply } => respond(engine, snapshot, reply, engine.pause(id).await),
        Request::Resume { id, reply } => respond(engine, snapshot, reply, engine.resume(id).await),
        Request::Remove { id, mode, reply } => {
            respond(engine, snapshot, reply, engine.remove(id, mode).await);
        }
    }
}

/// Publishes before replying, so a caller that sees success also sees its effect.
fn respond<E: Engine, T>(
    engine: &E,
    snapshot: &watch::Sender<Vec<TorrentStatus>>,
    reply: Reply<T>,
    result: Result<T, EngineError>,
) {
    if result.is_ok() {
        publish_if_changed(snapshot, engine.snapshot());
    }
    // The caller may have stopped waiting; the engine call is done either way.
    let _ = reply.send(result);
}

/// Publishing an equal list would wake every receiver for nothing.
fn publish_if_changed(snapshot: &watch::Sender<Vec<TorrentStatus>>, current: Vec<TorrentStatus>) {
    snapshot.send_if_modified(|published| {
        let changed = *published != current;
        if changed {
            *published = current;
        }
        changed
    });
}
