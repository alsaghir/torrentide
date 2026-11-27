//! Diagnostic logs for Torrentide.

use std::{
    fs,
    io::{self, IsTerminal},
    panic,
    path::PathBuf,
};

use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{InitError, RollingFileAppender, Rotation},
};
use tracing_subscriber::{
    EnvFilter, fmt as subscriber_fmt,
    layer::SubscriberExt,
    util::{SubscriberInitExt, TryInitError},
};

use crate::{APP_NAME, Paths};

// Target directives match by prefix, so all Torrentide crates get info while dependencies stay at warn.
const DEFAULT_DIRECTIVES: &str = "warn,torrentide=info";

/// Where [`init`] writes log files: `<state dir>/logs`.
#[must_use]
pub fn dir(paths: &Paths) -> PathBuf {
    paths.state_dir().join("logs")
}

/// Keeps the background log writer alive; dropping it flushes buffered events.
///
/// Hold it for the whole program.
#[must_use]
#[derive(Debug)]
pub struct LogGuard {
    _writer: WorkerGuard,
}

/// Diagnostic logging could not be initialised.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// The log directory could not be created.
    #[error("cannot create {}", .path.display())]
    CreateDir {
        /// Directory to create.
        path: PathBuf,
        /// Filesystem error.
        #[source]
        source: io::Error,
    },
    /// The rolling file appender could not start.
    #[error("cannot open log file in {}", .path.display())]
    File {
        /// Directory for the log files.
        path: PathBuf,
        /// Appender initialisation error.
        #[source]
        source: InitError,
    },
    /// A global subscriber or a `log` logger is already installed.
    #[error("logging is already initialised")]
    AlreadyInitialised(#[source] TryInitError),
}

/// Sends `tracing` events to a daily log file and to stderr, and logs panics.
///
/// `directives` use `RUST_LOG` syntax and replace the default filter, which
/// enables info and above for Torrentide and warnings and above elsewhere.
/// Invalid directives are logged as a warning and the default filter is used.
///
/// # Errors
///
/// Returns [`LogError`] if the log directory or file cannot be opened, or
/// if a global subscriber or `log` logger is already installed. An existing
/// `log` logger is detected after the global subscriber has been installed.
///
/// # Panics
///
/// Panics if the log writer thread cannot be spawned or if called from a panicking thread.
pub fn init(paths: &Paths, directives: Option<&str>) -> Result<LogGuard, LogError> {
    let log_dir = dir(paths);
    fs::create_dir_all(&log_dir).map_err(|source| LogError::CreateDir {
        path: log_dir.clone(),
        source,
    })?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(APP_NAME)
        .filename_suffix("log")
        // Bound disk use while keeping a week of daily logs for bug reports.
        .max_log_files(7)
        .build(&log_dir)
        .map_err(|source| LogError::File {
            path: log_dir,
            source,
        })?;
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let directives = directives.unwrap_or(DEFAULT_DIRECTIVES);
    let (filter, rejected) = match EnvFilter::try_new(directives) {
        Ok(filter) => (filter, None),
        Err(error) => (EnvFilter::new(DEFAULT_DIRECTIVES), Some(error)),
    };
    let file_layer = subscriber_fmt::layer().with_writer(writer).with_ansi(false);
    let stderr_layer = subscriber_fmt::layer()
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal());
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()
        .map_err(LogError::AlreadyInitialised)?;

    if let Some(error) = rejected {
        tracing::warn!(directives, %error, "invalid log directives; using defaults");
    }

    // Include panics in logs attached to bug reports without replacing the usual panic output.
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        tracing::error!("{info}");
        previous(info);
    }));

    Ok(LogGuard { _writer: guard })
}
