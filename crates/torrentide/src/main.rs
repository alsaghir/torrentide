//! Composition root of the Torrentide desktop app.

use std::{env, io, process::ExitCode, rc::Rc, time::Duration};

use tokio::runtime::Builder;
use torrentide_core::logging::{self, LogError, LogGuard};
use torrentide_core::{
    ClientHandle, Database, DatabaseError, EngineError, HomeNotFound, Paths, SettingsFile,
    SettingsFileError, error_chain,
};
use torrentide_engine_rqbit::RqbitEngine;
use torrentide_gui::{RunError, Window, columns};

/// How often the client polls for torrent status.
const TICK: Duration = Duration::from_secs(1);

#[derive(Debug, thiserror::Error)]
enum DesktopError {
    #[error(transparent)]
    HomeNotFound(#[from] HomeNotFound),
    #[error(transparent)]
    Logging(#[from] LogError),
    #[error("cannot start the async runtime")]
    Runtime(#[source] io::Error),
    #[error(transparent)]
    Settings(#[from] SettingsFileError),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Window(#[from] RunError),
}

fn main() -> ExitCode {
    let (paths, _log) = match startup() {
        Ok(started) => started,
        Err(error) => {
            eprintln!("torrentide: {}", error_chain(&error));
            return ExitCode::FAILURE;
        }
    };
    run(&paths).unwrap_or_else(|error| report_error(&error))
}

fn startup() -> Result<(Paths, LogGuard), DesktopError> {
    let paths = Paths::standard()?;
    let log = logging::init(&paths, env::var("RUST_LOG").ok().as_deref())?;
    Ok((paths, log))
}

fn run(paths: &Paths) -> Result<ExitCode, DesktopError> {
    // librqbit moves disk I/O off async threads via block_in_place, which needs a multi-thread runtime.
    let runtime = Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(DesktopError::Runtime)?;
    let client = runtime.block_on(start_client(paths))?;

    let exit = match run_window(&client) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report_error(&error.into()),
    };
    runtime.block_on(async { client.shutdown().await.stop().await });
    Ok(exit)
}

async fn start_client(paths: &Paths) -> Result<ClientHandle<RqbitEngine>, DesktopError> {
    let settings = SettingsFile::load_logged(paths)?;
    // A database open failure must not start the engine.
    let database = Database::open(paths)?;
    let engine = RqbitEngine::start(paths, &settings).await?;
    Ok(ClientHandle::spawn(engine, database, TICK)?)
}

fn run_window(client: &ClientHandle<RqbitEngine>) -> Result<(), RunError> {
    let window = Rc::new(Window::new(columns::builtin())?);
    window.follow(client.snapshot())?;
    window.run()
}

fn report_error(error: &DesktopError) -> ExitCode {
    tracing::error!("{}", error_chain(error));
    ExitCode::FAILURE
}
