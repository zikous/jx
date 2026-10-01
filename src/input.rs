//! Bytes in: files and stdin cut into chunks that each hold whole records, so a
//! chunk can be parsed on its own.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read};

use memchr::memrchr;

pub const STDIN_NAME: &str = "<stdin>";

const BLOCK: usize = 64 * 1024;
const CHUNK: usize = 256 * 1024;

/// How records are told apart in the input.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// JSON values, separated by whitespace or nothing.
    Json,
    /// One record per line.
    Lines,
}

/// A run of input that holds whole records.
pub struct Chunk {
    pub bytes: Vec<u8>,
    pub name: String,
    /// Newlines in the input before this chunk.
    pub first_line: usize,
}

/// Finds, as bytes arrive, the last place the input can be cut between records.
#[derive(Default)]
struct Scanner {
    scanned: usize,
    depth: usize,
    in_string: bool,
    escaped: bool,
    cut: Option<usize>,
}

impl Scanner {
    fn scan(&mut self, bytes: &[u8], framing: Framing) -> Option<usize> {
        if framing == Framing::Lines {
            return memrchr(b'\n', bytes).map(|i| i + 1);
        }
        for (i, &b) in bytes.iter().enumerate().skip(self.scanned) {
            if self.in_string {
                match (self.escaped, b) {
                    (true, _) => self.escaped = false,
                    (false, b'\\') => self.escaped = true,
                    (false, b'"') => self.in_string = false,
                    _ => {}
                }
                continue;
            }
            match b {
                b'"' => self.in_string = true,
                b'[' | b'{' => self.depth += 1,
                b']' | b'}' => self.depth = self.depth.saturating_sub(1),
                b'\n' if self.depth == 0 => self.cut = Some(i + 1),
                _ => {}
            }
        }
        self.scanned = bytes.len();
        self.cut
    }
}

/// Cuts files (or stdin, with no files) into chunks, one file after another.
pub struct Chunks {
    /// Files still to read; `None` is stdin.
    files: VecDeque<Option<String>>,
    reader: Option<Box<dyn Read + Send>>,
    name: String,
    framing: Framing,
    pending: Vec<u8>,
    scanner: Scanner,
    line: usize,
}

impl Chunks {
    pub fn new(files: &[String], framing: Framing) -> Self {
        let files = if files.is_empty() {
            VecDeque::from([None])
        } else {
            files.iter().cloned().map(Some).collect()
        };
        Self {
            files,
            reader: None,
            name: STDIN_NAME.into(),
            framing,
            pending: Vec::new(),
            scanner: Scanner::default(),
            line: 0,
        }
    }

    /// Opens the next file. One that cannot be opened is reported, and does not end the run.
    fn open_next(&mut self) -> Option<io::Result<()>> {
        let path = self.files.pop_front()?;
        self.line = 0;
        let Some(path) = path else {
            self.reader = Some(Box::new(io::stdin()));
            self.name = STDIN_NAME.into();
            return Some(Ok(()));
        };
        let opened = File::open(&path)
            .map_err(|err| io::Error::new(err.kind(), format!("Could not open {path}: {err}")));
        Some(opened.map(|file| {
            self.reader = Some(Box::new(file));
            self.name = path;
        }))
    }

    fn emit(&mut self, end: usize) -> Chunk {
        let rest = self.pending.split_off(end);
        let bytes = std::mem::replace(&mut self.pending, rest);
        let first_line = self.line;
        self.line += bytes.iter().filter(|&&b| b == b'\n').count();
        self.scanner = Scanner::default();
        Chunk {
            bytes,
            name: self.name.clone(),
            first_line,
        }
    }
}

impl Iterator for Chunks {
    type Item = io::Result<Chunk>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.reader.is_none()
                && let Err(err) = self.open_next()?
            {
                return Some(Err(err));
            }
            let start = self.pending.len();
            self.pending.resize(start + BLOCK, 0);
            let reader = self.reader.as_mut().expect("a file is open");
            let read = match reader.read(&mut self.pending[start..]) {
                Ok(read) => read,
                Err(err) => {
                    self.pending.truncate(start);
                    if err.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    self.reader = None;
                    return Some(Err(err));
                }
            };
            self.pending.truncate(start + read);
            if read == 0 {
                self.reader = None;
                if !self.pending.is_empty() {
                    return Some(Ok(self.emit(self.pending.len())));
                }
                continue;
            }
            // A short read means the source has run dry for now: deliver what is complete.
            let ready = self.pending.len() >= CHUNK || read < BLOCK;
            if let Some(end) = self
                .scanner
                .scan(&self.pending, self.framing)
                .filter(|_| ready)
            {
                return Some(Ok(self.emit(end)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut(text: &str, framing: Framing) -> Option<usize> {
        Scanner::default().scan(text.as_bytes(), framing)
    }

    #[test]
    fn json_is_cut_only_between_top_level_values() {
        assert_eq!(cut("{\"a\":\n1}\n{\"b\":", Framing::Json), Some(9));
        assert_eq!(cut("{\"a\":\"x\\\"\n\"", Framing::Json), None);
        assert_eq!(cut("1\n2\n3", Framing::Json), Some(4));
    }

    #[test]
    fn lines_are_cut_at_the_last_newline() {
        assert_eq!(cut("a\nb\nc", Framing::Lines), Some(4));
    }

    #[test]
    fn a_trickling_reader_still_yields_whole_values() {
        struct Trickle(Vec<u8>);
        impl Read for Trickle {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.0.len().min(3).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0.drain(..n);
                Ok(n)
            }
        }
        let mut chunks = Chunks::new(&[], Framing::Json);
        chunks.files.clear();
        chunks.reader = Some(Box::new(Trickle(b"{\"a\":1}\n[2]\n".to_vec())));
        let all: Vec<u8> = chunks.flat_map(|c| c.unwrap().bytes).collect();
        assert_eq!(all, b"{\"a\":1}\n[2]\n");
    }
}
