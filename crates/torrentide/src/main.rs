//! Composition root of the Torrentide desktop app.

use std::{env, process::ExitCode};

use torrentide::{App, StartError};
use torrentide_core::logging::{self, LogError, LogGuard};
use torrentide_core::{HomeNotFound, Paths, error_chain};
use torrentide_gui::RunError;

#[derive(Debug, thiserror::Error)]
enum DesktopError {
    #[error(transparent)]
    HomeNotFound(#[from] HomeNotFound),
    #[error(transparent)]
    Logging(#[from] LogError),
    #[error(transparent)]
    Start(#[from] StartError),
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
    let app = App::start(paths)?;
    let exit = match app.run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => report_error(&error.into()),
    };
    app.shutdown();
    Ok(exit)
}

fn report_error(error: &DesktopError) -> ExitCode {
    tracing::error!("{}", error_chain(error));
    ExitCode::FAILURE
}
