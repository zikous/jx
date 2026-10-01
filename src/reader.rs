//! A JSON parser over bytes that yields one value of a stream at a time.

use std::rc::Rc;

use memchr::{memchr2, memrchr};

use crate::value::{Map, Value};
use crate::writer;

const MAX_DEPTH: usize = 512;

#[derive(Debug)]
pub struct ParseError {
    pub offset: usize,
    pub message: &'static str,
}

type Res<T> = Result<T, ParseError>;

impl ParseError {
    /// `message at line L, column C`, with `bytes` starting at line `first_line`.
    pub fn describe(&self, bytes: &[u8], first_line: usize) -> String {
        let before = &bytes[..self.offset.min(bytes.len())];
        let line = first_line + before.iter().filter(|&&b| b == b'\n').count();
        let column = before.len() - memrchr(b'\n', before).map_or(0, |i| i + 1);
        let eof = if self.offset >= bytes.len() {
            " at EOF"
        } else {
            ""
        };
        format!("{}{eof} at line {line}, column {column}", self.message)
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        // A byte order mark in front of the text is not part of it.
        let pos = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            3
        } else {
            0
        };
        Self { bytes, pos }
    }

    /// Byte offset of the next unread byte.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Skips whitespace; true if nothing is left.
    fn at_end(&mut self) -> bool {
        self.skip_whitespace();
        self.pos == self.bytes.len()
    }

    pub fn next_value(&mut self) -> Res<Option<Value>> {
        if self.at_end() {
            return Ok(None);
        }
        self.value(0).map(Some)
    }

    /// Parses the next top-level value into `--stream` events instead of a tree:
    /// `[path, leaf]` for each leaf and `[path]` after each non-empty container.
    /// On a syntax error the events before it have already been emitted and
    /// `path` is where the parser stopped. Returns false at the end of input.
    pub fn next_events(&mut self, path: &mut Vec<Value>, emit: &mut dyn FnMut(Value)) -> Res<bool> {
        if self.at_end() {
            return Ok(false);
        }
        self.events(path, 0, emit)?;
        Ok(true)
    }

    fn events(
        &mut self,
        path: &mut Vec<Value>,
        depth: usize,
        emit: &mut dyn FnMut(Value),
    ) -> Res<()> {
        let event = |path: &[Value], leaf: Option<Value>| {
            let mut event = vec![Value::arr(path.to_vec())];
            event.extend(leaf);
            Value::arr(event)
        };
        let opener = self.peek();
        if !matches!(opener, Some(b'[' | b'{')) {
            let value = self.value(depth)?;
            emit(event(path, Some(value)));
            return Ok(());
        }
        let mut last = None;
        self.elements(depth, |reader, key, index| {
            let key = key.map_or(Value::num(index as f64), Value::Str);
            path.push(key.clone());
            reader.events(path, depth + 1, emit)?;
            path.pop();
            last = Some(key);
            Ok(())
        })?;
        match last {
            Some(key) => {
                path.push(key);
                emit(event(path, None));
                path.pop();
            }
            None if opener == Some(b'[') => emit(event(path, Some(Value::arr(Vec::new())))),
            None => emit(event(path, Some(Value::obj(Map::new())))),
        }
        Ok(())
    }

    /// Like `next_value`, but for a program that only reads the field chain
    /// `path`: everything else in each object along the way is checked, not built.
    pub fn next_projected(&mut self, path: &[String]) -> Res<Option<Value>> {
        if self.at_end() {
            return Ok(None);
        }
        self.projected(path, 0).map(Some)
    }

    fn projected(&mut self, path: &[String], depth: usize) -> Res<Value> {
        let Some((field, rest)) = path.split_first().filter(|_| self.peek() == Some(b'{')) else {
            return self.value(depth);
        };
        let mut map = Map::new();
        self.elements(depth, |reader, key, _| {
            let key = key.expect("objects have keys");
            if *key == **field {
                map.insert(key, reader.projected(rest, depth + 1)?);
            } else {
                reader.skip(depth + 1)?;
            }
            Ok(())
        })?;
        Ok(Value::obj(map))
    }

    /// Steps through the elements of the array or object at the cursor, calling
    /// `element` for each (with its key, for objects, already read).
    fn elements(
        &mut self,
        depth: usize,
        mut element: impl FnMut(&mut Self, Option<Rc<str>>, usize) -> Res<()>,
    ) -> Res<()> {
        if depth > MAX_DEPTH {
            return Err(self.error("Exceeds depth limit for parsing"));
        }
        let close = if self.peek() == Some(b'[') {
            b']'
        } else {
            b'}'
        };
        self.pos += 1;
        self.skip_whitespace();
        if self.eat(close) {
            return Ok(());
        }
        let mut index = 0;
        loop {
            self.skip_whitespace();
            let key = if close == b'}' {
                Some(self.key()?)
            } else {
                None
            };
            element(self, key, index)?;
            self.skip_whitespace();
            if self.eat(close) {
                return Ok(());
            }
            if !self.eat(b',') {
                return Err(self.unterminated_or("Expected separator between values"));
            }
            index += 1;
        }
    }

    /// An object key and the colon after it.
    fn key(&mut self) -> Res<Rc<str>> {
        if self.peek() != Some(b'"') {
            return Err(self.unterminated_or("Object keys must be strings"));
        }
        let key = self.string()?;
        self.skip_whitespace();
        if !self.eat(b':') {
            return Err(self.error("Objects must consist of key:value pairs"));
        }
        self.skip_whitespace();
        Ok(key)
    }

    /// Validates and steps over a value without building it.
    fn skip(&mut self, depth: usize) -> Res<()> {
        match self.peek() {
            Some(b'"') => self.skip_string(),
            Some(b'[' | b'{') => self.elements(depth, |reader, _, _| reader.skip(depth + 1)),
            _ => self.value(depth).map(drop),
        }
    }

    fn skip_string(&mut self) -> Res<()> {
        self.pos += 1;
        loop {
            let hit = memchr2(b'"', b'\\', &self.bytes[self.pos..])
                .ok_or_else(|| self.error("Unfinished string"))?;
            self.pos += hit;
            if self.bytes[self.pos] == b'"' {
                self.pos += 1;
                return Ok(());
            }
            self.pos = (self.pos + 2).min(self.bytes.len());
        }
    }

    /// `nan`, `NaN`, `Infinity` and their negations, which JSON does not allow.
    fn special_number(&mut self) -> Option<f64> {
        const WORDS: [(&[u8], f64); 6] = [
            (b"nan", f64::NAN),
            (b"NaN", f64::NAN),
            (b"-NaN", f64::NAN),
            (b"Infinity", f64::INFINITY),
            (b"-Infinity", f64::NEG_INFINITY),
            (b"-nan", f64::NAN),
        ];
        let rest = &self.bytes[self.pos..];
        let (word, number) = WORDS.iter().find(|(word, _)| {
            rest.starts_with(word)
                && rest
                    .get(word.len())
                    .is_none_or(|b| !b.is_ascii_alphanumeric())
        })?;
        self.pos += word.len();
        Some(*number)
    }

    fn value(&mut self, depth: usize) -> Res<Value> {
        let may_be_special = matches!(self.peek(), Some(b'n' | b'N' | b'I'))
            || (self.peek() == Some(b'-')
                && matches!(self.bytes.get(self.pos + 1), Some(b'N' | b'n' | b'I')));
        if may_be_special && let Some(number) = self.special_number() {
            return Ok(Value::num(number));
        }
        match self.peek() {
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::Str),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b'[') => {
                let mut items = Vec::new();
                self.elements(depth, |reader, _, _| {
                    items.push(reader.value(depth + 1)?);
                    Ok(())
                })?;
                Ok(Value::arr(items))
            }
            Some(b'{') => {
                let mut map = Map::new();
                self.elements(depth, |reader, key, _| {
                    map.insert(key.expect("objects have keys"), reader.value(depth + 1)?);
                    Ok(())
                })?;
                Ok(Value::obj(map))
            }
            Some(_) => Err(self.error("Invalid literal")),
            None => Err(self.error("Unfinished JSON term")),
        }
    }

    fn literal(&mut self, word: &[u8], value: Value) -> Res<Value> {
        let end = self.pos + word.len();
        let delimited = self
            .bytes
            .get(end)
            .is_none_or(|b| !b.is_ascii_alphanumeric());
        if !(self.bytes[self.pos..].starts_with(word) && delimited) {
            return Err(self.error("Invalid literal"));
        }
        self.pos = end;
        Ok(value)
    }

    fn number(&mut self) -> Res<Value> {
        let start = self.pos;
        self.eat(b'-');
        self.digits()?;
        if self.eat(b'.') {
            self.digits()?;
        }
        if self.eat(b'e') || self.eat(b'E') {
            let _ = self.eat(b'+') || self.eat(b'-');
            self.digits()?;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos]).expect("number is ascii");
        Ok(writer::number(text))
    }

    fn digits(&mut self) -> Res<()> {
        let count = self.bytes[self.pos..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if count == 0 {
            return Err(self.error("Invalid numeric literal"));
        }
        self.pos += count;
        Ok(())
    }

    fn string(&mut self) -> Res<Rc<str>> {
        let start = self.pos + 1;
        let rest = &self.bytes[start..];
        match memchr2(b'"', b'\\', rest) {
            Some(end) if rest[end] == b'"' => {
                self.pos = start + end + 1;
                Ok(String::from_utf8_lossy(&rest[..end]).as_ref().into())
            }
            Some(_) => self.escaped_string(start),
            None => Err(self.error("Unfinished string")),
        }
    }

    fn escaped_string(&mut self, start: usize) -> Res<Rc<str>> {
        let mut buf = Vec::new();
        let mut pos = start;
        loop {
            let run = memchr2(b'"', b'\\', &self.bytes[pos..])
                .ok_or_else(|| self.error("Unfinished string"))?;
            buf.extend_from_slice(&self.bytes[pos..pos + run]);
            pos += run;
            if self.bytes[pos] == b'"' {
                break;
            }
            let escape = *self
                .bytes
                .get(pos + 1)
                .ok_or_else(|| self.error("Unfinished string"))?;
            pos += 2;
            match escape {
                b'"' | b'\\' | b'/' => buf.push(escape),
                b'b' => buf.push(0x08),
                b'f' => buf.push(0x0c),
                b'n' => buf.push(b'\n'),
                b'r' => buf.push(b'\r'),
                b't' => buf.push(b'\t'),
                b'u' => {
                    let (ch, next) = self.unicode_escape(pos)?;
                    buf.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                    pos = next;
                }
                _ => return Err(self.error("Invalid escape")),
            }
        }
        self.pos = pos + 1;
        Ok(String::from_utf8_lossy(&buf).as_ref().into())
    }

    fn unicode_escape(&self, pos: usize) -> Res<(char, usize)> {
        let high = self.hex4(pos)?;
        let mut next = pos + 4;
        let code = match high {
            0xD800..=0xDBFF => {
                let low = self.bytes[next..]
                    .starts_with(b"\\u")
                    .then(|| self.hex4(next + 2))
                    .transpose()?
                    .filter(|low| (0xDC00..=0xDFFF).contains(low))
                    .ok_or_else(|| self.error("Invalid \\uXXXX\\uXXXX surrogate pair escape"))?;
                next += 6;
                0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
            }
            0xDC00..=0xDFFF => 0xFFFD,
            _ => high,
        };
        Ok((char::from_u32(code).unwrap_or('\u{FFFD}'), next))
    }

    fn hex4(&self, pos: usize) -> Res<u32> {
        let digits = self
            .bytes
            .get(pos..pos + 4)
            .ok_or_else(|| self.error("Invalid escape"))?;
        digits.iter().try_fold(0, |acc, &b| {
            let digit = (b as char)
                .to_digit(16)
                .ok_or_else(|| self.error("Invalid escape"))?;
            Ok(acc << 4 | digit)
        })
    }

    /// Running out of input inside a value is reported as an unfinished term.
    fn unterminated_or(&self, message: &'static str) -> ParseError {
        self.error(if self.pos >= self.bytes.len() {
            "Unfinished JSON term"
        } else {
            message
        })
    }

    fn skip_whitespace(&mut self) {
        let count = self.bytes[self.pos..]
            .iter()
            .take_while(|&&b| matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
            .count();
        self.pos += count;
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        self.pos += found as usize;
        found
    }

    fn error(&self, message: &'static str) -> ParseError {
        ParseError {
            offset: self.pos,
            message,
        }
    }
}

/// Parses `text` as exactly one JSON value.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader::new(text.as_bytes());
    let value = reader
        .next_value()
        .map_err(|e| e.describe(text.as_bytes(), 1))?;
    let value = value.ok_or("Expected JSON value")?;
    if !reader.at_end() {
        let extra = reader.error("Unexpected extra JSON values");
        return Err(extra.describe(text.as_bytes(), 1));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Value {
        parse(text).unwrap()
    }

    #[test]
    fn parses_escapes_and_surrogate_pairs() {
        assert_eq!(one(r#""a\né😀""#), Value::str("a\né😀"));
    }

    #[test]
    fn reports_where_a_value_is_malformed() {
        assert_eq!(
            parse("[1,2").unwrap_err(),
            "Unfinished JSON term at EOF at line 1, column 4"
        );
        assert_eq!(
            parse("{1:2}").unwrap_err(),
            "Object keys must be strings at line 1, column 1"
        );
        assert_eq!(
            parse("tru").unwrap_err(),
            "Invalid literal at line 1, column 0"
        );
    }

    #[test]
    fn deep_nesting_is_rejected_not_overflowed() {
        let text = "[".repeat(600);
        assert!(parse(&text).unwrap_err().starts_with("Exceeds depth limit"));
    }

    #[test]
    fn a_stream_yields_values_one_at_a_time() {
        let mut reader = Reader::new(b"1 [2] {\"a\":3}");
        let mut count = 0;
        while reader.next_value().unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 3);
    }
}
