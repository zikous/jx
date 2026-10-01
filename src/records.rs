//! Turns chunks into records: JSON values, lines or stream events.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::input::{Chunk, Framing, STDIN_NAME};
use crate::reader::{ParseError, Reader};
use crate::value::Value;

/// How records are read from the chunks.
#[derive(Clone, Copy)]
pub struct Reading {
    pub framing: Framing,
    /// Turn each document into `[path, leaf]` events.
    pub stream: bool,
}

/// Set when an input file could not be opened; the run goes on without it.
pub static OPEN_FAILED: AtomicBool = AtomicBool::new(false);

/// What reading one record from a chunk came to.
enum Step {
    /// A record, and the bytes it used.
    Value(Value, usize),
    /// Events were queued; the bytes they used.
    Queued(usize),
    Failed(ParseError),
    /// The chunk is used up.
    Done,
}

/// Turns chunks into records: JSON values, lines or stream events.
pub struct Records {
    chunks: Box<dyn Iterator<Item = io::Result<Chunk>>>,
    /// The fields the program reads; empty when it reads anything.
    projection: Vec<String>,
    reading: Reading,
    chunk: Option<Chunk>,
    pos: usize,
    name: Option<String>,
    queue: VecDeque<Value>,
    path: Vec<Value>,
    /// A syntax error found after some events, which are delivered first.
    pending: Option<String>,
    done: bool,
}

impl Records {
    pub fn new(
        chunks: impl Iterator<Item = io::Result<Chunk>> + 'static,
        reading: Reading,
        projection: Vec<String>,
    ) -> Self {
        Self {
            chunks: Box::new(chunks),
            projection,
            reading,
            chunk: None,
            pos: 0,
            name: None,
            queue: VecDeque::new(),
            path: Vec::new(),
            pending: None,
            done: false,
        }
    }

    pub fn location(&self) -> String {
        format!(
            "{}:{}",
            self.name.as_deref().unwrap_or(STDIN_NAME),
            self.line()
        )
    }

    /// Lines consumed so far, counting the one that ends the last record.
    pub fn line(&self) -> usize {
        let Some(chunk) = &self.chunk else { return 0 };
        let end = self.pos.min(chunk.bytes.len());
        self.lines_before(chunk) + usize::from(chunk.bytes.get(end) == Some(&b'\n'))
    }

    /// Newlines in the input before the current position.
    fn lines_before(&self, chunk: &Chunk) -> usize {
        let end = self.pos.min(chunk.bytes.len());
        chunk.first_line + chunk.bytes[..end].iter().filter(|&&b| b == b'\n').count()
    }

    pub fn filename(&self) -> Option<String> {
        self.name.clone()
    }

    /// Like `pull`, for `input` and `inputs`: errors say where they happened.
    pub fn next_input(&mut self) -> Option<Result<Value, String>> {
        let next = self.pull()?;
        Some(next.map_err(|message| format!("{message} at {}", self.location())))
    }

    /// The next record, or a syntax error, after which there are no more.
    /// Files that cannot be read are reported and skipped.
    pub fn pull(&mut self) -> Option<Result<Value, String>> {
        loop {
            if let Some(event) = self.queue.pop_front() {
                return Some(Ok(event));
            }
            if let Some(message) = self.pending.take() {
                self.done = true;
                return Some(Err(message));
            }
            if self.done {
                return None;
            }
            if self.chunk.is_none() {
                match self.chunks.next()? {
                    Ok(chunk) => {
                        self.name = Some(chunk.name.clone());
                        (self.chunk, self.pos) = (Some(chunk), 0);
                    }
                    Err(err) => {
                        report_open_failure(&err.to_string());
                        continue;
                    }
                }
            }
            match self.record() {
                Some(Ok(value)) => return Some(Ok(value)),
                Some(Err(message)) => self.pending = Some(message),
                None => {}
            }
        }
    }

    /// The next record of the current chunk; `None` once the chunk is used up
    /// (events are queued instead of returned).
    fn record(&mut self) -> Option<Result<Value, String>> {
        let chunk = self.chunk.as_ref().expect("a chunk is loaded");
        let bytes = &chunk.bytes[self.pos..];
        let step = match self.reading.framing {
            Framing::Lines => parse_line(bytes),
            Framing::Json if self.reading.stream => {
                parse_events(bytes, &mut self.queue, &mut self.path)
            }
            Framing::Json => parse_value(bytes, &self.projection),
        };
        match step {
            Step::Value(value, used) => {
                self.pos += used;
                Some(Ok(value))
            }
            Step::Queued(used) => {
                self.pos += used;
                None
            }
            Step::Failed(err) => Some(Err(err.describe(bytes, self.lines_before(chunk) + 1))),
            Step::Done => {
                self.chunk = None;
                None
            }
        }
    }
}

fn parse_line(bytes: &[u8]) -> Step {
    if bytes.is_empty() {
        return Step::Done;
    }
    let end = bytes
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(bytes.len());
    Step::Value(
        Value::str(String::from_utf8_lossy(&bytes[..end])),
        (end + 1).min(bytes.len()),
    )
}

fn parse_events(bytes: &[u8], queue: &mut VecDeque<Value>, path: &mut Vec<Value>) -> Step {
    let mut reader = Reader::new(bytes);
    match reader.next_events(path, &mut |event| queue.push_back(event)) {
        Ok(true) => Step::Queued(reader.position()),
        Ok(false) => Step::Done,
        Err(err) => Step::Failed(err),
    }
}

fn parse_value(bytes: &[u8], projection: &[String]) -> Step {
    let mut reader = Reader::new(bytes);
    let next = if projection.is_empty() {
        reader.next_value()
    } else {
        reader.next_projected(projection)
    };
    match next {
        Ok(Some(value)) => Step::Value(value, reader.position()),
        Ok(None) => Step::Done,
        Err(err) => Step::Failed(err),
    }
}

pub fn report_open_failure(message: &str) {
    tracing::error!("jx: error: {message}");
    OPEN_FAILED.store(true, Ordering::Relaxed);
}
