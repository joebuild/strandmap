use std::process::ExitCode;

use clap::Parser;
use strandmap::{cli::Cli, commands};

fn main() -> ExitCode {
    match commands::run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}
