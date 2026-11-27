//! Composition root of the Torrentide desktop app.

use std::process::ExitCode;

use torrentide_gui::{Window, columns};

fn main() -> ExitCode {
    match Window::new(columns::builtin()).and_then(|window| window.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("torrentide: {}", torrentide_core::error_chain(&error));
            ExitCode::FAILURE
        }
    }
}
