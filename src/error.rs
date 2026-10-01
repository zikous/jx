//! What can stop a run before the program starts evaluating: a program that
//! does not parse, bad command-line arguments, or unreadable files. Each says
//! what went wrong and where. Errors raised while a program runs are values,
//! handled by the evaluator.

use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("syntax error, unexpected {found} at <top-level>, line {line}")]
    Unexpected { found: String, line: usize },

    #[error("{message} at <top-level>, line {line}")]
    Syntax { message: &'static str, line: usize },

    #[error("{name}/{arity} is not defined at <top-level>, line {line}")]
    UndefinedFunction {
        name: String,
        arity: usize,
        line: usize,
    },

    #[error("${name} is not defined at <top-level>, line {line}")]
    UndefinedVariable { name: String, line: usize },

    #[error("$*label-{name} is not defined at <top-level>, line {line}")]
    UndefinedLabel { name: String, line: usize },

    #[error("Top-level program not given (try \".\")")]
    NoMainExpression,

    #[error("no program given (try \".\")")]
    NoProgram,

    #[error("cannot read program file {path}: {source}")]
    ProgramFile { path: String, source: io::Error },

    #[error("cannot indent more than 7 characters")]
    IndentTooWide,

    #[error("invalid JSON for {what}: {reason}")]
    InvalidJson { what: String, reason: String },

    #[error("cannot read {path} for {flag} {name}: {reason}")]
    ArgumentFile {
        flag: &'static str,
        name: String,
        path: String,
        reason: String,
    },

    #[error(transparent)]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn is_broken_pipe(&self) -> bool {
        matches!(self, Self::Io(err) if err.kind() == io::ErrorKind::BrokenPipe)
    }

    /// The process exit code: 3 when the program is invalid, 2 for anything
    /// wrong with the arguments or files.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Unexpected { .. }
            | Self::Syntax { .. }
            | Self::UndefinedFunction { .. }
            | Self::UndefinedVariable { .. }
            | Self::UndefinedLabel { .. }
            | Self::NoMainExpression => 3,
            _ => 2,
        }
    }
}
