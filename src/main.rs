mod builtins;
mod cli;
mod error;
mod eval;
mod input;
mod library;
mod parallel;
mod parser;
mod reader;
mod records;
mod run;
mod stack;
mod value;
mod writer;

use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .without_time()
        .with_target(false)
        .with_level(false)
        .with_ansi(false)
        .init();
    let settings = match cli::from_args() {
        Ok(settings) => settings,
        Err(err) if err.is_broken_pipe() => return ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("jx: error: {err}");
            return ExitCode::from(err.exit_code());
        }
    };
    match stacker::maybe_grow(stack::RED_ZONE, stack::GROWTH, || run::run(&settings)) {
        Ok(code) => ExitCode::from(code as u8),
        Err(err) if err.is_broken_pipe() => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("jx: error: {err}");
            ExitCode::from(err.exit_code())
        }
    }
}
