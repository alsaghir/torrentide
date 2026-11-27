//! Headless front end: the same core as the GUI, driven from a terminal or a script.

use std::borrow::Cow;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use tokio::sync::watch;
use torrentide_core::{
    AddOptions, AddSource, ClientError, ClientHandle, Database, DatabaseError, EngineError,
    HomeNotFound, LoadedSettings, Paths, Settings, SettingsFile, SettingsFileError, Torrent,
    TorrentState,
};
use torrentide_engine_rqbit::RqbitEngine;

/// How often the client publishes torrent status.
const TICK: Duration = Duration::from_secs(1);

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Keep every file under this directory instead of the standard user directories.
    #[arg(long, global = true, value_name = "DIR")]
    portable: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Lists the torrents in queue order: position, info hash and added time (Unix seconds),
    /// tab-separated.
    List,
    /// Adds a `.torrent` file and prints its info hash; it starts with the next `run`.
    Add {
        file: PathBuf,
        /// Keep the torrent paused until resumed.
        #[arg(long)]
        paused: bool,
    },
    /// Runs the client in the foreground until interrupted or terminated, printing every torrent
    /// (info hash and state, tab-separated) whenever one changes; an empty line ends each list.
    Run,
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    HomeNotFound(#[from] HomeNotFound),
    #[error(transparent)]
    Settings(#[from] SettingsFileError),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error("cannot read {}", .path.display())]
    ReadTorrent {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot write output")]
    Output(#[source] io::Error),
    #[error("cannot install signal handlers")]
    Signal(#[source] io::Error),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("torrentide-cli: {}", torrentide_core::error_chain(&error));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), CliError> {
    let paths = paths(cli.portable.as_deref())?;
    match cli.command {
        Command::List => list(&paths),
        Command::Add { file, paused } => add(&paths, file, paused).await,
        Command::Run => run_client(&paths).await,
    }
}

fn paths(portable: Option<&Path>) -> Result<Paths, HomeNotFound> {
    portable.map_or_else(Paths::standard, |root| Ok(Paths::portable(root)))
}

fn list(paths: &Paths) -> Result<(), CliError> {
    let torrents = Database::open(paths)?.torrents()?;
    let mut out = io::stdout().lock();
    torrents
        .iter()
        .try_for_each(|torrent| {
            writeln!(
                out,
                "{}\t{}\t{}",
                torrent.queue_position, torrent.id, torrent.added_at
            )
        })
        .map_err(CliError::Output)
}

async fn add(paths: &Paths, file: PathBuf, paused: bool) -> Result<(), CliError> {
    let torrent = fs::read(&file).map_err(|source| CliError::ReadTorrent { path: file, source })?;
    let (client, settings) = client(paths).await?;
    let added = client
        .add(
            AddSource::TorrentFile(torrent),
            AddOptions {
                save_path: settings.downloads.dir,
                start_paused: paused,
            },
        )
        .await;
    client.shutdown().await.stop().await;
    println!("{}", added?);
    Ok(())
}

/// Starts the client the way the GUI does: settings, database, engine.
async fn client(paths: &Paths) -> Result<(ClientHandle<RqbitEngine>, Settings), CliError> {
    let settings = settings(paths)?;
    // Before the engine: opening fails without side effects.
    let database = Database::open(paths)?;
    let engine = RqbitEngine::start(paths, &settings).await?;
    let client = ClientHandle::spawn(engine, database, TICK)?;
    Ok((client, settings))
}

fn settings(paths: &Paths) -> Result<Settings, SettingsFileError> {
    let file = SettingsFile::new(paths);
    let settings = match file.load_or_init(|| Settings::first_run(paths))? {
        LoadedSettings::Existing(settings) => settings,
        LoadedSettings::Created(settings) => {
            eprintln!("wrote default settings to {}", file.path().display());
            settings
        }
        LoadedSettings::Recovered {
            settings,
            invalid,
            reason,
        } => {
            eprintln!(
                "settings were invalid ({}), moved to {}; using defaults",
                torrentide_core::error_chain(&reason),
                invalid.display()
            );
            settings
        }
    };
    Ok(settings)
}

async fn run_client(paths: &Paths) -> Result<(), CliError> {
    // Installed before anything is printed, so a signal sent after the first output is caught.
    let stop = shutdown_signal().map_err(CliError::Signal)?;
    let (client, _) = client(paths).await?;
    let printed = print_changes(client.snapshot(), stop).await;
    client.shutdown().await.stop().await;
    printed.map_err(CliError::Output)
}

/// Prints the snapshot, then again on every change, until `stop` resolves or the client ends.
async fn print_changes(
    mut snapshot: watch::Receiver<Vec<Torrent>>,
    stop: impl Future<Output = ()>,
) -> io::Result<()> {
    let mut stop = pin!(stop);
    loop {
        // Cloned so the client is not held up by a slow terminal while printing.
        let torrents = snapshot.borrow_and_update().clone();
        print_torrents(&torrents)?;
        tokio::select! {
            changed = snapshot.changed() => {
                // An error means the client ended; shutting it down reports why.
                if changed.is_err() {
                    return Ok(());
                }
            }
            () = &mut stop => return Ok(()),
        }
    }
}

fn print_torrents(torrents: &[Torrent]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    for torrent in torrents {
        let status = &torrent.status;
        writeln!(out, "{}\t{}", status.id, state(&status.state))?;
    }
    writeln!(out)
}

fn state(state: &TorrentState) -> Cow<'_, str> {
    match state {
        TorrentState::Checking => "checking".into(),
        TorrentState::Paused => "paused".into(),
        TorrentState::Running => "running".into(),
        TorrentState::Failed(reason) => format!("failed: {reason}").into(),
    }
}

/// Resolves on the first SIGINT, SIGTERM or SIGHUP (terminal closed).
#[cfg(unix)]
fn shutdown_signal() -> io::Result<impl Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let mut hangup = signal(SignalKind::hangup())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
            _ = hangup.recv() => {}
        }
    })
}

/// Resolves on the first Ctrl+C.
#[cfg(windows)]
fn shutdown_signal() -> io::Result<impl Future<Output = ()>> {
    let mut ctrl_c = tokio::signal::windows::ctrl_c()?;
    Ok(async move {
        ctrl_c.recv().await;
    })
}
