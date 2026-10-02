//! Runs a program over the input and works out the exit code: compiles it,
//! feeds it records on one thread or all cores, and writes the output in input
//! order.

use std::io::{self, BufWriter, Write};
use std::iter;
use std::sync::atomic::Ordering;

use crate::error::Result;
use crate::eval::{Environment, Program, Stop, error, error_line};
use crate::input::{Chunk, Chunks, Framing};
use crate::parallel::{Flow, run_ordered};
use crate::records::{OPEN_FAILED, Reading, Records, report_open_failure};
use crate::value::Value;
use crate::writer::{Format, write_value};

/// What the program runs on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `null`, without reading input unless the program asks for it.
    Null,
    /// Every record of the input, in parallel.
    Each,
    /// All records gathered into a single array (or, for lines, one string).
    Slurp,
}

/// Everything a run needs. Shared between threads, so it holds no values.
pub struct Settings {
    pub program: String,
    pub environment: Environment,
    pub recursion_limit: Option<usize>,
    pub files: Vec<String>,
    pub mode: Mode,
    pub reading: Reading,
    pub format: Format,
    pub exit_status: bool,
    /// Flush after every chunk of output instead of when the buffer fills.
    pub flush_each: bool,
}

/// What running the program over some records produced, to be written in order.
#[derive(Default)]
struct Batch {
    out: Vec<u8>,
    errors: Vec<String>,
    outputs: bool,
    last_truthy: bool,
    errored: bool,
    halt: Option<(i32, Option<String>)>,
}

impl Batch {
    /// Runs the program on one value; false if the whole run must stop.
    fn execute(
        &mut self,
        program: &Program,
        value: Value,
        location: &dyn Fn() -> String,
        settings: &Settings,
    ) -> bool {
        let result = program.run(value, &mut |output| {
            write_value(&mut self.out, &output, &settings.format).map_err(error)?;
            (self.outputs, self.last_truthy) = (true, output.truthy());
            Ok(())
        });
        match result {
            Ok(()) | Err(Stop::Passthrough) => true,
            Err(Stop::Error(value)) => {
                self.errors.push(error_line(&value, &location()));
                self.errored = true;
                true
            }
            Err(Stop::Break(_)) => {
                self.errors
                    .push(format!("jx: error (at {}): break", location()));
                self.errored = true;
                true
            }
            Err(Stop::Halt { code, message }) => {
                self.halt = Some((code, message));
                false
            }
        }
    }

    fn fail(&mut self, message: String) {
        self.errors.push(format!("jx: error: {message}"));
        self.errored = true;
    }
}

fn run_chunk(program: &Program, chunk: Chunk, settings: &Settings, projection: &[String]) -> Batch {
    let mut batch = Batch::default();
    let mut records = Records::new(iter::once(Ok(chunk)), settings.reading, projection.to_vec());
    while let Some(next) = records.pull() {
        match next {
            Ok(value) => {
                if !batch.execute(program, value, &|| records.location(), settings) {
                    break;
                }
            }
            Err(message) => {
                batch.fail(format!("(at {}): {message}", records.location()));
                break;
            }
        }
    }
    batch
}

/// Writes batches in order and keeps what the exit code depends on.
struct Sink<'o> {
    out: BufWriter<io::StdoutLock<'o>>,
    flush_each: bool,
    failure: Option<io::Error>,
    outputs: bool,
    last_truthy: bool,
    errored: bool,
    halt: Option<i32>,
}

impl<'o> Sink<'o> {
    fn new(stdout: io::StdoutLock<'o>, flush_each: bool) -> Self {
        Self {
            out: BufWriter::with_capacity(64 * 1024, stdout),
            flush_each,
            failure: None,
            outputs: false,
            last_truthy: false,
            errored: false,
            halt: None,
        }
    }

    /// Writes one batch of output and folds it into the tally.
    fn take(&mut self, batch: Batch) -> Flow {
        if let Err(err) = self.write(&batch.out) {
            self.failure = Some(err);
            return Flow::Stop;
        }
        batch
            .errors
            .iter()
            .for_each(|line| tracing::error!("{line}"));
        if batch.outputs {
            (self.outputs, self.last_truthy) = (true, batch.last_truthy);
        }
        self.errored |= batch.errored;
        let Some((code, message)) = batch.halt else {
            return Flow::Continue;
        };
        if let Some(message) = message {
            let _ = self.out.flush();
            eprint!("{message}");
        }
        self.halt = Some(code);
        Flow::Stop
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        if self.flush_each {
            self.out.flush()
        } else {
            Ok(())
        }
    }

    fn finish(mut self, exit_status: bool) -> Result<i32> {
        if let Some(err) = self.failure.take() {
            return Err(err.into());
        }
        self.out.flush()?;
        Ok(self.exit_code(exit_status))
    }

    fn exit_code(&self, exit_status: bool) -> i32 {
        if let Some(code) = self.halt {
            code
        } else if self.errored {
            5
        } else if OPEN_FAILED.load(Ordering::Relaxed) {
            2
        } else if exit_status && !self.outputs {
            4
        } else if exit_status && !self.last_truthy {
            1
        } else {
            0
        }
    }
}

pub fn run(settings: &Settings) -> Result<i32> {
    let program = Program::compile(
        &settings.program,
        &settings.environment,
        settings.recursion_limit,
    )?;
    let stdout = io::stdout();
    let mut sink = Sink::new(stdout.lock(), settings.flush_each);
    let chunks = Chunks::new(&settings.files, settings.reading.framing);

    // Hands the input to the program, for `input` and `inputs`.
    let read_ahead = |chunks| {
        *program.inputs.borrow_mut() = Some(Records::new(chunks, settings.reading, Vec::new()));
    };
    let run_once = |sink: &mut Sink, input| {
        let mut batch = Batch::default();
        batch.execute(&program, input, &|| "<unknown>".into(), settings);
        sink.take(batch);
    };
    match settings.mode {
        Mode::Null => {
            read_ahead(chunks);
            run_once(&mut sink, Value::Null);
        }
        Mode::Slurp if settings.reading.framing == Framing::Lines => {
            run_once(&mut sink, Value::str(read_all_text(chunks)));
        }
        Mode::Slurp => {
            read_ahead(chunks);
            match read_all_values(&program) {
                Ok(values) => run_once(&mut sink, values),
                Err(message) => {
                    let mut batch = Batch::default();
                    batch.fail(message);
                    sink.take(batch);
                }
            }
        }
        // Records are read on this thread, shared with `input` and `inputs`.
        Mode::Each if program.uses_inputs => {
            read_ahead(chunks);
            while let Some(next) = program.next_input() {
                let mut batch = Batch::default();
                let alive = match next {
                    Ok(value) => {
                        batch.execute(&program, value, &|| program.input_location(), settings)
                    }
                    Err(message) => {
                        batch.fail(message);
                        false
                    }
                };
                if matches!(sink.take(batch), Flow::Stop) || !alive {
                    break;
                }
            }
        }
        // Chunks go to whichever core is free, and are written back in input order.
        Mode::Each => {
            let projection = program.field_path().unwrap_or_default();
            let new_worker = || {
                let program = Program::compile(
                    &settings.program,
                    &settings.environment,
                    settings.recursion_limit,
                )
                .expect("the program compiled already");
                let projection = &projection;
                move |chunk: Chunk| run_chunk(&program, chunk, settings, projection)
            };
            run_ordered(chunks, new_worker, |result| match result {
                Ok(batch) => sink.take(batch),
                Err(err) => {
                    report_open_failure(&err.to_string());
                    Flow::Continue
                }
            });
        }
    }
    sink.finish(settings.exit_status)
}

fn read_all_values(program: &Program) -> std::result::Result<Value, String> {
    let mut values = Vec::new();
    while let Some(next) = program.next_input() {
        values.push(next?);
    }
    Ok(Value::arr(values))
}

fn read_all_text(chunks: Chunks) -> String {
    let mut text = Vec::new();
    for chunk in chunks {
        match chunk {
            Ok(chunk) => text.extend(chunk.bytes),
            Err(err) => report_open_failure(&err.to_string()),
        }
    }
    String::from_utf8_lossy(&text).into_owned()
}
