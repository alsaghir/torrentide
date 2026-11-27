//! Composition root of the Torrentide desktop app.

use std::process::ExitCode;

fn main() -> ExitCode {
    match torrentide_gui::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("torrentide: {}", torrentide_core::error_chain(&error));
            ExitCode::FAILURE
        }
    }
}
