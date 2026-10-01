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
mod value;
mod writer;

use std::io;
use std::process::ExitCode;

/// Programs recurse deeply; give the evaluating thread room for it.
const STACK_SIZE: usize = 512 << 20;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .without_time()
        .with_target(false)
        .with_level(false)
        .with_ansi(false)
        .init();
    let result = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(|| run::run(&cli::from_args()?))
        .expect("failed to start the evaluator thread")
        .join();
    match result {
        Ok(Ok(code)) => ExitCode::from(code as u8),
        Ok(Err(err)) if err.is_broken_pipe() => ExitCode::SUCCESS,
        Ok(Err(err)) => {
            tracing::error!("jx: error: {err}");
            ExitCode::from(err.exit_code())
        }
        Err(_) => ExitCode::from(2),
    }
}
