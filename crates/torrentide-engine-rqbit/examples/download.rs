//! Downloads one torrent with default session options (DHT on, no incoming listener).
//!
//! `cargo run --release --example download -- <magnet | URL | .torrent path> <output dir>`

use std::{error::Error, path::PathBuf};

use librqbit::{AddTorrent, AddTorrentOptions, Session};
use torrentide_engine_rqbit::download;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let (Some(source), Some(output), None) = (args.next(), args.next(), args.next()) else {
        return Err("usage: download <magnet | URL | .torrent path> <output dir>".into());
    };

    let session = Session::new(PathBuf::from(output)).await?;
    download(
        &session,
        AddTorrent::from_cli_argument(&source)?,
        AddTorrentOptions::default(),
    )
    .await?;
    println!("downloaded");
    Ok(())
}
