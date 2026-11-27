//! Composes the desktop window, client and engine.

use std::{fmt, io, rc::Rc, time::Duration};

use tokio::runtime::{Builder, Runtime};
use torrentide_core::{
    ClientHandle, Database, DatabaseError, EngineError, Paths, SettingsFile, SettingsFileError,
};
use torrentide_engine_rqbit::RqbitEngine;
use torrentide_gui::{RunError, Window, columns};

/// How often the client polls for torrent status.
const TICK: Duration = Duration::from_secs(1);

/// The desktop application could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The async runtime could not be created.
    #[error("cannot start the async runtime")]
    Runtime(#[source] io::Error),
    /// The settings could not be loaded.
    #[error(transparent)]
    Settings(#[from] SettingsFileError),
    /// The database could not be opened or reconciled.
    #[error(transparent)]
    Database(#[from] DatabaseError),
    /// The engine could not be initialized.
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// The window could not be created or connected to snapshots.
    #[error(transparent)]
    Window(#[from] RunError),
}

/// A desktop window and its running torrent client.
pub struct App {
    runtime: Runtime,
    client: ClientHandle<RqbitEngine>,
    window: Rc<Window>,
}

impl fmt::Debug for App {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("App")
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

impl App {
    /// Starts the client and connects the window to its torrent snapshots.
    ///
    /// Call on the window thread, outside an async runtime.
    ///
    /// # Errors
    ///
    /// Returns [`StartError`] if any part of the application fails to start.
    ///
    /// # Panics
    ///
    /// Panics if called from within an async runtime.
    pub fn start(paths: &Paths) -> Result<Self, StartError> {
        // librqbit moves disk I/O off async threads via block_in_place, which needs a multi-thread runtime.
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(StartError::Runtime)?;
        let client = runtime.block_on(start_client(paths))?;
        let window = match start_window(&client) {
            Ok(window) => window,
            Err(error) => {
                stop_client(&runtime, client);
                return Err(error.into());
            }
        };
        Ok(Self {
            runtime,
            client,
            window,
        })
    }

    /// Returns the desktop window.
    #[must_use]
    pub fn window(&self) -> &Window {
        &self.window
    }

    /// Returns the running client, for example to add torrents.
    #[must_use]
    pub fn client(&self) -> &ClientHandle<RqbitEngine> {
        &self.client
    }

    /// Shows the window and runs its event loop until it closes.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] if the window or event loop fails.
    pub fn run(&self) -> Result<(), RunError> {
        self.window.run()
    }

    /// Blocks until the client and engine have stopped.
    ///
    /// # Panics
    ///
    /// Panics if called from within an async runtime or if the client panicked.
    pub fn shutdown(self) {
        stop_client(&self.runtime, self.client);
    }
}

async fn start_client(paths: &Paths) -> Result<ClientHandle<RqbitEngine>, StartError> {
    let settings = SettingsFile::load_logged(paths)?;
    // A database open failure must not start the engine.
    let database = Database::open(paths)?;
    let engine = RqbitEngine::start(paths, &settings).await?;
    Ok(ClientHandle::spawn(engine, database, TICK)?)
}

fn start_window(client: &ClientHandle<RqbitEngine>) -> Result<Rc<Window>, RunError> {
    let window = Window::new(columns::builtin())?;
    window.follow(client.snapshot())?;
    Ok(window)
}

fn stop_client(runtime: &Runtime, client: ClientHandle<RqbitEngine>) {
    runtime.block_on(async { client.shutdown().await.stop().await });
}
