//! The command line: what the options mean, and the settings they produce.

use std::fs;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use clap::{CommandFactory, FromArgMatches, Parser, parser::ValueSource};

use crate::error::{Error, Result};
use crate::eval::Environment;
use crate::input::Framing;
use crate::reader::Reader;
use crate::records::Reading;
use crate::run::{Mode, Settings};
use crate::value::Value;
use crate::writer::{Format, Indent, dump};

/// A fast JSON processor.
#[derive(Parser)]
#[command(name = "jx", version, about, disable_help_subcommand = true)]
struct Cli {
    /// The filter to run (or the first file, with --from-file).
    #[arg(allow_hyphen_values = true)]
    program: Option<String>,

    /// Input files; stdin when none. With --args or --jsonargs, positional arguments instead.
    files: Vec<String>,

    /// Use null as the input instead of reading any.
    #[arg(short = 'n', long)]
    null_input: bool,

    /// Read each line as a string instead of parsing JSON.
    #[arg(short = 'R', long)]
    raw_input: bool,

    /// Read all inputs into one array.
    #[arg(short = 's', long)]
    slurp: bool,

    /// Print one value per line, without indentation.
    #[arg(short = 'c', long, id = "compact")]
    compact_output: bool,

    /// Print strings without quotes.
    #[arg(short = 'r', long)]
    raw_output: bool,

    /// Like -r, with nothing between outputs.
    #[arg(short = 'j', long)]
    join_output: bool,

    /// Like -r, with a NUL byte after each output.
    #[arg(long)]
    raw_output0: bool,

    /// Escape every non-ASCII character.
    #[arg(short = 'a', long)]
    ascii_output: bool,

    /// Print object keys in sorted order.
    #[arg(short = 'S', long)]
    sort_keys: bool,

    /// Color the output.
    #[arg(short = 'C', long)]
    color_output: bool,

    /// Never color the output.
    #[arg(short = 'M', long)]
    monochrome_output: bool,

    /// Indent with a tab.
    #[arg(long, id = "tab")]
    tab: bool,

    /// Indent with this many spaces (at most 7).
    #[arg(long, id = "indent", value_name = "N")]
    indent: Option<u8>,

    /// Flush the output after every value.
    #[arg(long)]
    unbuffered: bool,

    /// Turn each document into `[path, leaf]` events.
    #[arg(long)]
    stream: bool,

    /// Read the filter from a file.
    #[arg(short = 'f', long, value_name = "FILE")]
    from_file: Option<PathBuf>,

    /// Set the exit status from the last output.
    #[arg(short = 'e', long)]
    exit_status: bool,

    /// Set $NAME to a string.
    #[arg(long, num_args = 2, value_names = ["NAME", "VALUE"], allow_hyphen_values = true)]
    arg: Vec<String>,

    /// Set $NAME to JSON text.
    #[arg(long, num_args = 2, value_names = ["NAME", "TEXT"], allow_hyphen_values = true)]
    argjson: Vec<String>,

    /// Set $NAME to an array of the values in a file.
    #[arg(long, num_args = 2, value_names = ["NAME", "FILE"])]
    slurpfile: Vec<String>,

    /// Set $NAME to the contents of a file, as a string.
    #[arg(long, num_args = 2, value_names = ["NAME", "FILE"])]
    rawfile: Vec<String>,

    /// Treat the remaining positional arguments as strings in $ARGS.positional.
    #[arg(long)]
    args: bool,

    /// Treat the remaining positional arguments as JSON in $ARGS.positional.
    #[arg(long)]
    jsonargs: bool,
}

/// Parses the process arguments into settings, or exits with usage help.
pub fn from_args() -> Result<Settings> {
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|err| err.exit());
    let last = |id: &str| {
        matches
            .value_source(id)
            .filter(|source| *source == ValueSource::CommandLine)
            .and_then(|_| matches.indices_of(id)?.max())
    };
    // The last of -c, --tab and --indent wins.
    let layouts = [
        (last("compact"), Indent::Compact),
        (last("tab"), Indent::Tab),
        (last("indent"), Indent::Spaces(cli.indent.unwrap_or(2))),
    ];
    let indent = layouts
        .into_iter()
        .filter_map(|(at, indent)| Some((at?, indent)))
        .max_by_key(|(at, _)| *at)
        .map_or(Indent::Spaces(2), |(_, indent)| indent);
    settings(cli, indent)
}

fn settings(cli: Cli, indent: Indent) -> Result<Settings> {
    let named = named_variables(&cli)?;
    let (program, files) = match &cli.from_file {
        Some(path) => (
            fs::read_to_string(path).map_err(|source| Error::ProgramFile {
                path: path.display().to_string(),
                source,
            })?,
            cli.program.into_iter().chain(cli.files).collect(),
        ),
        None => (cli.program.ok_or(Error::NoProgram)?, cli.files),
    };
    // With --args or --jsonargs, what would be files are values for $ARGS.positional.
    let (files, positional) = match (cli.args, cli.jsonargs) {
        (false, false) => (files, Vec::new()),
        (_, true) => (Vec::new(), files),
        (true, false) => (
            Vec::new(),
            files.iter().map(|p| dump(&Value::str(p))).collect(),
        ),
    };

    let tty = io::stdout().is_terminal();
    let colored = cli.color_output
        || (tty && !cli.monochrome_output && std::env::var_os("NO_COLOR").is_none());
    let indent = match indent {
        Indent::Spaces(0) => Indent::Compact,
        Indent::Spaces(8..) => return Err(Error::IndentTooWide),
        indent => indent,
    };
    let format = Format {
        indent,
        raw: cli.raw_output || cli.join_output || cli.raw_output0,
        ascii: cli.ascii_output,
        sort_keys: cli.sort_keys,
        terminator: match (cli.raw_output0, cli.join_output) {
            (true, _) => Some(0),
            (false, true) => None,
            (false, false) => Some(b'\n'),
        },
        colors: colored && !cli.monochrome_output,
    };
    let framing = if cli.raw_input {
        Framing::Lines
    } else {
        Framing::Json
    };
    let mode = match (cli.null_input, cli.slurp) {
        (true, _) => Mode::Null,
        (false, true) => Mode::Slurp,
        (false, false) => Mode::Each,
    };
    Ok(Settings {
        program,
        environment: Environment { named, positional },
        files,
        mode,
        reading: Reading {
            framing,
            stream: cli.stream,
        },
        format,
        exit_status: cli.exit_status,
        flush_each: cli.unbuffered || tty,
    })
}

/// The `--arg`, `--argjson`, `--slurpfile` and `--rawfile` variables, each as
/// its name and JSON text.
fn named_variables(cli: &Cli) -> Result<Vec<(String, String)>> {
    let mut named = Vec::new();
    for pair in cli.arg.chunks(2) {
        named.push((pair[0].clone(), dump(&Value::str(&pair[1]))));
    }
    for pair in cli.argjson.chunks(2) {
        named.push((pair[0].clone(), pair[1].clone()));
    }
    for pair in cli.slurpfile.chunks(2) {
        let text = read_argument_file("--slurpfile", pair)?;
        let mut reader = Reader::new(&text);
        let mut values = Vec::new();
        while let Some(value) = reader
            .next_value()
            .map_err(|err| argument_file_error("--slurpfile", pair, err.describe(&text, 1)))?
        {
            values.push(value);
        }
        named.push((pair[0].clone(), dump(&Value::arr(values))));
    }
    for pair in cli.rawfile.chunks(2) {
        let text = read_argument_file("--rawfile", pair)?;
        named.push((
            pair[0].clone(),
            dump(&Value::str(String::from_utf8_lossy(&text))),
        ));
    }
    Ok(named)
}

fn read_argument_file(flag: &'static str, pair: &[String]) -> Result<Vec<u8>> {
    fs::read(&pair[1]).map_err(|err| argument_file_error(flag, pair, err.to_string()))
}

fn argument_file_error(flag: &'static str, pair: &[String], reason: String) -> Error {
    Error::ArgumentFile {
        flag,
        name: pair[0].clone(),
        path: pair[1].clone(),
        reason,
    }
}
