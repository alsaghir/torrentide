//! Headless front end: the same core as the GUI, driven from a terminal or a script.

use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use torrentide_core::{
    AddOptions, AddSource, ClientError, ClientHandle, Database, DatabaseError, EngineError,
    HomeNotFound, LoadedSettings, Paths, Settings, SettingsFile, SettingsFileError,
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
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("torrentide-cli: {}", chain(&error));
            ExitCode::FAILURE
        }
    }
}

/// The error and its sources, outermost first: `cannot read x: No such file or directory`.
fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

async fn run(cli: Cli) -> Result<(), CliError> {
    let paths = paths(cli.portable.as_deref())?;
    match cli.command {
        Command::List => list(&paths),
        Command::Add { file, paused } => add(&paths, file, paused).await,
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
                chain(&reason),
                invalid.display()
            );
            settings
        }
    };
    Ok(settings)
}
