//! Writing values as text: indentation, colors, escaping and number forms.

use std::rc::Rc;

use crate::value::{Value, sorted};

const RESET: &str = "\x1b[0m";

#[derive(Clone, Copy, Default)]
pub enum Indent {
    #[default]
    Compact,
    Spaces(u8),
    Tab,
}

/// The default is compact JSON, as `tojson` produces.
#[derive(Clone, Default)]
pub struct Format {
    pub indent: Indent,
    /// Print strings without quotes or escapes.
    pub raw: bool,
    /// Escape every non-ASCII character.
    pub ascii: bool,
    pub sort_keys: bool,
    /// Byte written after each output.
    pub terminator: Option<u8>,
    pub colors: bool,
}

/// Escape sequences for null, false, true, numbers, strings, arrays, objects and keys.
const COLORS: [&str; 8] = [
    "\x1b[0;90m",
    "\x1b[0;39m",
    "\x1b[0;39m",
    "\x1b[0;39m",
    "\x1b[0;32m",
    "\x1b[1;39m",
    "\x1b[1;39m",
    "\x1b[1;34m",
];

/// Compact JSON text, as `tojson` produces.
pub fn dump(value: &Value) -> String {
    let mut out = Vec::new();
    write_json(&mut out, value, &Format::default(), 0);
    String::from_utf8(out).expect("output is valid UTF-8")
}

/// Appends one output value, including its terminator.
pub fn write_value(out: &mut Vec<u8>, value: &Value, format: &Format) -> Result<(), String> {
    match value {
        Value::Str(s) if format.raw && !format.ascii => {
            if format.terminator == Some(0) && s.contains('\0') {
                return Err("Cannot dump a string containing NUL with --raw-output0 option".into());
            }
            out.extend_from_slice(s.as_bytes());
        }
        _ => write_json(out, value, format, 0),
    }
    out.extend(format.terminator);
    Ok(())
}

fn write_json(out: &mut Vec<u8>, value: &Value, format: &Format, depth: usize) {
    let color = format.colors.then(|| COLORS[value.rank() as usize]);
    if let Some(color) = color {
        out.extend_from_slice(color.as_bytes());
    }
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(flag) => out.extend_from_slice(if *flag { b"true" } else { b"false" }),
        Value::Num(_, Some(text)) => out.extend_from_slice(text.as_bytes()),
        Value::Num(n, None) => out.extend_from_slice(float_text(*n).as_bytes()),
        Value::Str(s) => write_string(out, s, format.ascii),
        Value::Arr(items) if items.is_empty() => out.extend_from_slice(b"[]"),
        Value::Arr(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                separator(out, color, i);
                newline(out, format.indent, depth + 1);
                write_json(out, item, format, depth + 1);
            }
            close(out, format, color, depth, b']');
        }
        Value::Obj(map) if map.is_empty() => out.extend_from_slice(b"{}"),
        Value::Obj(map) => {
            out.push(b'{');
            let entries: Vec<_> = if format.sort_keys {
                sorted(map)
            } else {
                map.iter().collect()
            };
            for (i, (key, item)) in entries.into_iter().enumerate() {
                separator(out, color, i);
                newline(out, format.indent, depth + 1);
                if let Some(color) = color {
                    out.extend_from_slice(COLORS[7].as_bytes());
                    write_string(out, key, format.ascii);
                    out.extend_from_slice(format!("{RESET}{color}:{RESET}").as_bytes());
                } else {
                    write_string(out, key, format.ascii);
                    out.push(b':');
                }
                if !matches!(format.indent, Indent::Compact) {
                    out.push(b' ');
                }
                write_json(out, item, format, depth + 1);
            }
            close(out, format, color, depth, b'}');
        }
    }
    if color.is_some() {
        out.extend_from_slice(RESET.as_bytes());
    }
}

fn separator(out: &mut Vec<u8>, color: Option<&str>, index: usize) {
    if let Some(color) = color {
        if index > 0 {
            out.extend_from_slice(color.as_bytes());
            out.push(b',');
        }
        out.extend_from_slice(RESET.as_bytes());
    } else if index > 0 {
        out.push(b',');
    }
}

fn close(out: &mut Vec<u8>, format: &Format, color: Option<&str>, depth: usize, bracket: u8) {
    newline(out, format.indent, depth);
    if let Some(color) = color {
        out.extend_from_slice(color.as_bytes());
    }
    out.push(bracket);
}

fn newline(out: &mut Vec<u8>, indent: Indent, depth: usize) {
    let (fill, count) = match indent {
        Indent::Compact => return,
        Indent::Spaces(width) => (b' ', depth * width as usize),
        Indent::Tab => (b'\t', depth),
    };
    out.push(b'\n');
    out.resize(out.len() + count, fill);
}

fn write_string(out: &mut Vec<u8>, s: &str, ascii: bool) {
    out.push(b'"');
    for ch in s.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            c if c < ' ' || c == '\u{7f}' || (ascii && !c.is_ascii()) => {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    out.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
                }
            }
            c => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    out.push(b'"');
}

/// The shortest digits that round-trip, in plain notation unless the
/// exponent is very small or very large.
pub fn float_text(value: f64) -> String {
    if value.is_nan() {
        return "null".into();
    }
    let value = value.clamp(-f64::MAX, f64::MAX);
    let sign = if value.is_sign_negative() { "-" } else { "" };
    if value == 0.0 {
        return format!("{sign}0");
    }
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').expect("{:e} has an exponent");
    let exponent: i32 = exponent.parse().expect("exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let (count, point) = (digits.len() as i32, exponent + 1);
    let body = if point <= -4 || point > count + 15 {
        let rest = if count > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{}{rest}e{sign}{:02}", &digits[..1], exponent.abs())
    } else if point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else if point >= count {
        format!("{digits}{}", "0".repeat((point - count) as usize))
    } else {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    };
    format!("{sign}{body}")
}

/// A number read from text: a double, plus its canonical text when that
/// differs from how the double prints. Literals without an exponent are kept
/// verbatim; others use the decimal "scientific string" form (`1e2` -> `1E+2`).
pub fn number(text: &str) -> Value {
    if text.starts_with('.') || text.starts_with("-.") {
        return number(&text.replacen('.', "0.", 1));
    }
    let digits = text.strip_prefix('-').unwrap_or(text);
    let plain = text.len() <= 15
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (digits.len() == 1 && text != "-0" || !digits.starts_with('0') && !digits.is_empty());
    let float: f64 = text.parse().unwrap_or(f64::NAN);
    if plain {
        return Value::num(float);
    }
    let canonical = canonical(text);
    if canonical == float_text(float) {
        Value::num(float)
    } else {
        Value::Num(float, Some(Rc::from(canonical)))
    }
}

fn canonical(text: &str) -> String {
    let Some((mantissa, exponent)) = text.split_once(['e', 'E']) else {
        return text.to_string();
    };
    let Ok(exponent) = exponent.parse::<i64>() else {
        return text.to_string();
    };
    let (sign, mantissa) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |m| ("-", m));
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all = [int, frac].concat();
    let digits = match all.trim_start_matches('0') {
        "" => "0",
        digits => digits,
    };
    let exponent = exponent - frac.len() as i64;
    let adjusted = exponent + digits.len() as i64 - 1;
    if exponent <= 0 && adjusted >= -6 {
        let point = digits.len() as i64 + exponent;
        if point > 0 {
            let (whole, fraction) = digits.split_at(point as usize);
            let dot = if fraction.is_empty() { "" } else { "." };
            format!("{sign}{whole}{dot}{fraction}")
        } else {
            format!(
                "{sign}0.{}{digits}",
                "0".repeat(point.unsigned_abs() as usize)
            )
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let dot = if rest.is_empty() { "" } else { "." };
        let exp_sign = if adjusted < 0 { '-' } else { '+' };
        format!(
            "{sign}{first}{dot}{rest}E{exp_sign}{}",
            adjusted.unsigned_abs()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computed_numbers_use_shortest_digits() {
        assert_eq!(float_text(0.1 + 0.2), "0.30000000000000004");
        assert_eq!(float_text(1e17), "1e+17");
        assert_eq!(float_text(1e-5), "1e-05");
        assert_eq!(float_text(-0.0), "-0");
    }

    #[test]
    fn literals_keep_their_text() {
        assert_eq!(dump(&number("1.50")), "1.50");
        assert_eq!(dump(&number("1E2")), "1E+2");
        assert_eq!(dump(&number("42")), "42");
        assert_eq!(
            dump(&number("12345678901234567890")),
            "12345678901234567890"
        );
    }
}
