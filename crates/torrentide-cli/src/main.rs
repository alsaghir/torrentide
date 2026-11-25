//! Headless front end: the same core as the GUI, driven from a terminal or a script.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use torrentide_core::{Database, DatabaseError, HomeNotFound, Paths};

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
}

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    HomeNotFound(#[from] HomeNotFound),
    #[error(transparent)]
    Database(#[from] DatabaseError),
    #[error("cannot write output")]
    Output(#[source] io::Error),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("torrentide-cli: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), CliError> {
    let paths = paths(cli.portable.as_deref())?;
    match cli.command {
        Command::List => list(&paths),
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
