//! The function library: strings, regular expressions and encodings, math,
//! and dates. Each family is a table of `(name, arity, function)` for the
//! builtin registry.

use crate::builtins::Table;
use crate::reader;
use crate::value::{self, Map, Res, Value, describe, type_error, type_error2};
use crate::writer::{self, dump};
use data_encoding::{BASE32, BASE64, BASE64_NOPAD};
use fancy_regex::Regex;
use libc::{c_char, time_t, tm};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use std::ffi::{CStr, CString};
use std::time::{SystemTime, UNIX_EPOCH};

/// Everything except letters, digits and `-_.~` is escaped by `@uri`.
const URI_RESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub const STRINGS: Table = &[
    ("tojson", 0, tojson),
    ("fromjson", 0, fromjson),
    ("tonumber", 0, tonumber),
    ("tostring", 0, tostring),
    ("format", 1, format_by_name),
    ("join", 1, join),
    ("utf8bytelength", 0, utf8bytelength),
    ("startswith", 1, startswith),
    ("endswith", 1, endswith),
    ("explode", 0, explode),
    ("implode", 0, implode),
    ("split", 1, split),
    ("ascii_downcase", 0, |v| {
        ascii(v, "ascii_downcase", str::to_ascii_lowercase)
    }),
    ("ascii_upcase", 0, |v| {
        ascii(v, "ascii_upcase", str::to_ascii_uppercase)
    }),
    ("_strindices", 1, string_indices),
    ("_match_impl", 3, match_impl),
];

/// A string as itself and any other value as compact JSON.
pub fn to_string(value: &Value) -> String {
    match value {
        Value::Str(s) => s.to_string(),
        other => dump(other),
    }
}

fn tojson(v: &[Value]) -> Res<Value> {
    Ok(Value::str(dump(&v[0])))
}

fn tostring(v: &[Value]) -> Res<Value> {
    Ok(match &v[0] {
        Value::Str(_) => v[0].clone(),
        other => Value::str(dump(other)),
    })
}

fn fromjson(v: &[Value]) -> Res<Value> {
    let Value::Str(text) = &v[0] else {
        return Err(type_error(&v[0], "cannot be parsed as JSON"));
    };
    reader::parse(text).map_err(|message| format!("{message} (while parsing '{text}')"))
}

fn tonumber(v: &[Value]) -> Res<Value> {
    match &v[0] {
        Value::Num(..) => Ok(v[0].clone()),
        Value::Str(s) => {
            let text = s.trim();
            let valid = !text.is_empty() && text.bytes().all(|b| b"0123456789+-.eE".contains(&b));
            if valid && text.parse::<f64>().is_ok() {
                Ok(writer::number(text.strip_prefix('+').unwrap_or(text)))
            } else if text == "nan" {
                Ok(Value::num(f64::NAN))
            } else {
                Err(format!(
                    "Invalid numeric literal at EOF at line 1, column {} (while parsing '{s}')",
                    s.len()
                ))
            }
        }
        other => Err(type_error(other, "cannot be parsed as a number")),
    }
}

fn string<'a>(value: &'a Value, message: &str) -> Res<&'a str> {
    value.as_str().ok_or_else(|| message.to_string())
}

fn join(v: &[Value]) -> Res<Value> {
    let items: Vec<&Value> = match &v[0] {
        Value::Arr(items) => items.iter().collect(),
        Value::Obj(map) => map.values().collect(),
        other => return Err(format!("Cannot iterate over {}", describe(other))),
    };
    let mut joined = String::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            joined.push_str(string(
                &v[1],
                &type_error2(&Value::str(&joined), &v[1], "cannot be added"),
            )?);
        }
        match item {
            Value::Null => {}
            Value::Str(s) => joined.push_str(s),
            Value::Bool(_) | Value::Num(..) => joined.push_str(&dump(item)),
            other => return Err(type_error2(&Value::str(&joined), other, "cannot be added")),
        }
    }
    Ok(Value::str(joined))
}

fn utf8bytelength(v: &[Value]) -> Res<Value> {
    let s = string(
        &v[0],
        &type_error(&v[0], "only strings have UTF-8 byte length"),
    )?;
    Ok(Value::num(s.len() as f64))
}

fn startswith(v: &[Value]) -> Res<Value> {
    match (&v[0], &v[1]) {
        (Value::Str(s), Value::Str(prefix)) => Ok(Value::Bool(s.starts_with(&prefix[..]))),
        _ => Err("startswith() requires string inputs".into()),
    }
}

fn endswith(v: &[Value]) -> Res<Value> {
    match (&v[0], &v[1]) {
        (Value::Str(s), Value::Str(suffix)) => Ok(Value::Bool(s.ends_with(&suffix[..]))),
        _ => Err("endswith() requires string inputs".into()),
    }
}

fn ascii(v: &[Value], name: &str, convert: fn(&str) -> String) -> Res<Value> {
    let Some(s) = v[0].as_str() else {
        return Err(format!("{name} input must be a string"));
    };
    Ok(Value::str(convert(s)))
}

fn explode(v: &[Value]) -> Res<Value> {
    let s = string(&v[0], "explode input must be a string")?;
    Ok(Value::arr(
        s.chars()
            .map(|c| Value::num(f64::from(u32::from(c))))
            .collect(),
    ))
}

fn implode(v: &[Value]) -> Res<Value> {
    let Value::Arr(codes) = &v[0] else {
        return Err("implode input must be an array".into());
    };
    let chars: Option<String> = codes
        .iter()
        .map(|code| {
            code.as_f64().map(|n| {
                u32::try_from(n as i64)
                    .ok()
                    .and_then(char::from_u32)
                    .unwrap_or('\u{FFFD}')
            })
        })
        .collect();
    chars
        .map(Value::str)
        .ok_or_else(|| "Invalid codepoint literal".into())
}

fn split(v: &[Value]) -> Res<Value> {
    match (&v[0], &v[1]) {
        (Value::Str(text), Value::Str(sep)) => Ok(Value::arr(value::split(text, sep))),
        _ => Err("split input and separator must be strings".into()),
    }
}

/// Positions of every (overlapping) occurrence of `v[1]` in `v[0]`, in characters.
fn string_indices(v: &[Value]) -> Res<Value> {
    let (Value::Str(text), Value::Str(needle)) = (&v[0], &v[1]) else {
        return Err("Cannot determine indices of a non-string".into());
    };
    if needle.is_empty() {
        return Ok(Value::arr(Vec::new()));
    }
    let positions = text
        .char_indices()
        .enumerate()
        .filter(|(_, (byte, _))| text[*byte..].starts_with(&needle[..]))
        .map(|(position, _)| Value::num(position as f64))
        .collect();
    Ok(Value::arr(positions))
}

fn match_impl(v: &[Value]) -> Res<Value> {
    let Value::Str(text) = &v[0] else {
        return Err(type_error(
            &v[0],
            "cannot be matched, as it is not a string",
        ));
    };
    let Value::Str(pattern) = &v[1] else {
        return Err(type_error(
            &v[1],
            "cannot be matched, as it is not a string",
        ));
    };
    let flags = match &v[2] {
        Value::Null => "",
        Value::Str(flags) => flags,
        other => return Err(format!("{} is not a string", describe(other))),
    };
    let test = v[3].truthy();
    let text: &str = text;

    let (mut global, mut prefix, mut skip_empty) = (false, String::new(), false);
    for flag in flags.chars() {
        match flag {
            'g' => global = true,
            'i' => prefix.push('i'),
            'x' => prefix.push('x'),
            's' => prefix.push('s'),
            'p' => prefix.push('s'),
            'n' => skip_empty = true,
            'l' => {}
            _ => return Err(format!("{flags} is not a valid modifier string")),
        }
    }
    let source = if prefix.is_empty() {
        pattern.to_string()
    } else {
        format!("(?{prefix}){pattern}")
    };
    let regex = Regex::new(&source)
        .map_err(|e| format!("{pattern} (at offset 0) is not a valid regex: {e}"))?;
    let names: Vec<Option<String>> = regex.capture_names().map(|n| n.map(String::from)).collect();

    let (mut matches, mut from) = (Vec::new(), 0);
    while from <= text.len() {
        let found = regex
            .captures_from_pos(text, from)
            .map_err(|e| e.to_string())?;
        let Some(captures) = found else { break };
        let whole = captures.get(0).expect("group 0 always participates");
        if !(skip_empty && whole.start() == whole.end()) {
            if test {
                return Ok(Value::Bool(true));
            }
            matches.push(describe_match(&captures, &names, text));
        }
        if !global {
            break;
        }
        from = match text[whole.end()..].chars().next() {
            Some(c) if whole.start() == whole.end() => whole.end() + c.len_utf8(),
            Some(_) => whole.end(),
            None => break,
        };
    }
    Ok(if test {
        Value::Bool(false)
    } else {
        Value::arr(matches)
    })
}

fn describe_match(
    captures: &fancy_regex::Captures<'_, str>,
    names: &[Option<String>],
    text: &str,
) -> Value {
    let group = |index: usize| {
        let (offset, length, string) = match captures.get(index) {
            Some(m) => (
                text[..m.start()].chars().count() as f64,
                m.as_str().chars().count() as f64,
                Value::str(m.as_str()),
            ),
            None => (-1.0, 0.0, Value::Null),
        };
        Map::from_iter([
            ("offset".into(), Value::num(offset)),
            ("length".into(), Value::num(length)),
            ("string".into(), string),
        ])
    };
    let mut whole = group(0);
    let groups = (1..names.len())
        .map(|i| {
            let mut entry = group(i);
            entry.insert(
                "name".into(),
                names[i].as_deref().map_or(Value::Null, Value::str),
            );
            Value::obj(entry)
        })
        .collect();
    whole.insert("captures".into(), Value::arr(groups));
    Value::obj(whole)
}

fn format_by_name(v: &[Value]) -> Res<Value> {
    let name = string(&v[1], &type_error(&v[1], "is not a valid format"))?;
    format(name, &v[0]).map(Value::str)
}

/// Applies `@name` to a value.
pub fn format(name: &str, value: &Value) -> Res<String> {
    match name {
        "text" => Ok(to_string(value)),
        "json" => Ok(dump(value)),
        "csv" => delimited(value, "csv"),
        "tsv" => delimited(value, "tsv"),
        "html" => Ok(to_string(value)
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\'', "&apos;")
            .replace('"', "&quot;")),
        "uri" => Ok(utf8_percent_encode(&to_string(value), URI_RESERVED).to_string()),
        "urid" => Ok(percent_decode_str(&to_string(value))
            .decode_utf8_lossy()
            .into_owned()),
        "sh" => shell_quote(value),
        "base64" => Ok(BASE64.encode(to_string(value).as_bytes())),
        "base64d" => decode_base64(&to_string(value)),
        "base32" => Ok(BASE32.encode(to_string(value).as_bytes())),
        "base32d" => decode(
            BASE32.decode(to_string(value).as_bytes()),
            "string is not valid base32 data",
        ),
        other => Err(format!("{other} is not a valid format")),
    }
}

fn delimited(value: &Value, kind: &str) -> Res<String> {
    let Value::Arr(items) = value else {
        return Err(format!(
            "{} cannot be {kind}-formatted, only array",
            describe(value)
        ));
    };
    let fields: Res<Vec<String>> = items
        .iter()
        .map(|item| match item {
            Value::Null => Ok(String::new()),
            Value::Bool(_) | Value::Num(..) => Ok(dump(item)),
            Value::Str(s) if kind == "csv" => Ok(format!("\"{}\"", s.replace('"', "\"\""))),
            Value::Str(s) => Ok(s
                .replace('\\', "\\\\")
                .replace('\t', "\\t")
                .replace('\n', "\\n")
                .replace('\r', "\\r")),
            other => Err(format!("{} is not valid in a {kind} row", describe(other))),
        })
        .collect();
    Ok(fields?.join(if kind == "csv" { "," } else { "\t" }))
}

fn shell_quote(value: &Value) -> Res<String> {
    let quote = |item: &Value| match item {
        Value::Str(s) => Ok(format!("'{}'", s.replace('\'', "'\\''"))),
        Value::Arr(_) | Value::Obj(_) => {
            Err(format!("{} can not be escaped for shell", describe(item)))
        }
        other => Ok(dump(other)),
    };
    match value {
        Value::Arr(items) => Ok(items.iter().map(quote).collect::<Res<Vec<_>>>()?.join(" ")),
        other => quote(other),
    }
}

fn decode_base64(text: &str) -> Res<String> {
    decode(
        BASE64_NOPAD.decode(text.trim_end_matches('=').as_bytes()),
        &format!("{} is not valid base64 data", describe(&Value::str(text))),
    )
}

fn decode(bytes: Result<Vec<u8>, data_encoding::DecodeError>, what: &str) -> Res<String> {
    let bytes = bytes.map_err(|_| what.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn number(value: &Value) -> Res<f64> {
    value
        .as_f64()
        .ok_or_else(|| type_error(value, "number required"))
}

macro_rules! unary {
    ($($name:ident => $function:expr),* $(,)?) => {
        $(fn $name(v: &[Value]) -> Res<Value> {
            let function: fn(f64) -> f64 = $function;
            Ok(Value::num(function(number(&v[0])?)))
        })*
        pub const UNARY: Table = &[$((stringify!($name), 0, $name)),*];
    };
}

macro_rules! binary {
    ($($name:ident => $function:expr),* $(,)?) => {
        $(fn $name(v: &[Value]) -> Res<Value> {
            let function: fn(f64, f64) -> f64 = $function;
            Ok(Value::num(function(number(&v[1])?, number(&v[2])?)))
        })*
        pub const BINARY: Table = &[$((stringify!($name), 2, $name)),*];
    };
}

unary! {
    floor => f64::floor, ceil => f64::ceil, round => f64::round, rint => f64::round_ties_even,
    trunc => f64::trunc, fabs => f64::abs, sqrt => f64::sqrt, cbrt => f64::cbrt,
    exp => f64::exp, exp2 => f64::exp2, exp10 => |x| 10f64.powf(x), expm1 => f64::exp_m1,
    log => f64::ln, log2 => f64::log2, log10 => f64::log10, log1p => f64::ln_1p,
    sin => f64::sin, cos => f64::cos, tan => f64::tan,
    asin => f64::asin, acos => f64::acos, atan => f64::atan,
    sinh => f64::sinh, cosh => f64::cosh, tanh => f64::tanh,
    asinh => f64::asinh, acosh => f64::acosh, atanh => f64::atanh,
}

binary! {
    atan2 => f64::atan2, pow => f64::powf, fmod => |x, y| x % y, hypot => f64::hypot,
    fmin => f64::min, fmax => f64::max, copysign => f64::copysign,
}

pub const OTHER: Table = &[
    ("infinite", 0, |_| Ok(Value::num(f64::INFINITY))),
    ("nan", 0, |_| Ok(Value::num(f64::NAN))),
    ("isinfinite", 0, |v| {
        Ok(Value::Bool(number(&v[0])?.is_infinite()))
    }),
    ("isnan", 0, |v| Ok(Value::Bool(number(&v[0])?.is_nan()))),
    ("isnormal", 0, |v| {
        Ok(Value::Bool(number(&v[0])?.is_normal()))
    }),
];

pub const DATES: Table = &[
    ("now", 0, now),
    ("mktime", 0, mktime),
    ("gmtime", 0, |v| broken_down(&v[0], false)),
    ("localtime", 0, |v| broken_down(&v[0], true)),
    ("strftime", 1, |v| strftime(v, false)),
    ("strflocaltime", 1, |v| strftime(v, true)),
    ("strptime", 1, strptime),
];

fn now(_: &[Value]) -> Res<Value> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?;
    Ok(Value::num(elapsed.as_secs_f64()))
}

fn blank() -> tm {
    // SAFETY: `tm` is plain data; all-zero is a valid value (null zone pointer included).
    unsafe { std::mem::zeroed() }
}

/// The time as `tm`, in UTC or the local zone.
fn to_tm(seconds: f64, local: bool) -> Res<tm> {
    if !seconds.is_finite() {
        return Err("error converting number of seconds since epoch to datetime".into());
    }
    let (t, mut out) = (seconds.floor() as time_t, blank());
    // SAFETY: both pointers refer to live, properly typed values.
    let done = unsafe {
        if local {
            libc::localtime_r(&t, &mut out)
        } else {
            libc::gmtime_r(&t, &mut out)
        }
    };
    if done.is_null() {
        Err("error converting number of seconds since epoch to datetime".into())
    } else {
        Ok(out)
    }
}

fn parts(t: &tm, fraction: f64) -> Value {
    let fields = [
        f64::from(t.tm_year) + 1900.0,
        f64::from(t.tm_mon),
        f64::from(t.tm_mday),
        f64::from(t.tm_hour),
        f64::from(t.tm_min),
        f64::from(t.tm_sec) + fraction,
        f64::from(t.tm_wday),
        f64::from(t.tm_yday),
    ];
    Value::arr(fields.into_iter().map(Value::num).collect())
}

fn broken_down(value: &Value, local: bool) -> Res<Value> {
    let Some(seconds) = value.as_f64() else {
        return Err(format!(
            "{}() requires a number",
            if local { "localtime" } else { "gmtime" }
        ));
    };
    Ok(parts(&to_tm(seconds, local)?, seconds - seconds.floor()))
}

/// A `tm` from a broken-down time array, normalised (and its zone filled in) by `timegm`.
fn from_parts(value: &Value, what: &str) -> Res<tm> {
    let Value::Arr(items) = value else {
        return Err(format!("{what} requires array of 6 numbers"));
    };
    let numbers: Option<Vec<f64>> = items.iter().map(Value::as_f64).collect();
    let Some(n) = numbers.filter(|n| n.len() >= 6) else {
        return Err(format!("{what} requires parsed datetime inputs"));
    };
    let mut t = blank();
    t.tm_year = (n[0] - 1900.0) as i32;
    t.tm_mon = n[1] as i32;
    t.tm_mday = n[2] as i32;
    t.tm_hour = n[3] as i32;
    t.tm_min = n[4] as i32;
    t.tm_sec = n[5] as i32;
    Ok(t)
}

fn utc_seconds(mut t: tm) -> f64 {
    // SAFETY: `t` is a live, properly typed value.
    unsafe { libc::timegm(&mut t) as f64 }
}

fn mktime(v: &[Value]) -> Res<Value> {
    if !matches!(v[0], Value::Arr(_)) {
        return Err("mktime requires array inputs".into());
    }
    Ok(Value::num(utc_seconds(from_parts(&v[0], "mktime")?)))
}

fn strftime(v: &[Value], local: bool) -> Res<Value> {
    let name = if local {
        "strflocaltime/1"
    } else {
        "strftime/1"
    };
    let Value::Str(format) = &v[1] else {
        return Err(format!("{name} requires a string format"));
    };
    let t = match &v[0] {
        Value::Num(seconds, _) => to_tm(*seconds, local)?,
        Value::Arr(_) => {
            let seconds = utc_seconds(from_parts(&v[0], name)?);
            to_tm(seconds, local)?
        }
        other => {
            return Err(format!(
                "{name} requires parsed datetime inputs: {}",
                type_error(other, "")
            ));
        }
    };
    let format = CString::new(format.as_bytes())
        .map_err(|_| format!("{name}: format contains a NUL byte"))?;
    let mut buffer = vec![0u8; 4096];
    // SAFETY: the buffer length is passed along, and both strings are NUL-terminated.
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr().cast::<c_char>(),
            buffer.len(),
            format.as_ptr(),
            &t,
        )
    };
    if written == 0 && !format.as_bytes().is_empty() {
        return Err(format!("{name}: unknown system failure"));
    }
    buffer.truncate(written);
    Ok(Value::str(String::from_utf8_lossy(&buffer)))
}

fn strptime(v: &[Value]) -> Res<Value> {
    let (Value::Str(text), Value::Str(format)) = (&v[0], &v[1]) else {
        return Err("strptime/1 requires string inputs and arguments".to_string());
    };
    let mismatch = || format!("date \"{text}\" does not match format \"{format}\"");
    let (input, pattern) = (
        CString::new(text.as_bytes()).map_err(|_| mismatch())?,
        CString::new(format.as_bytes()).map_err(|_| mismatch())?,
    );
    let mut t = blank();
    // SAFETY: both strings are NUL-terminated and `t` is a live `tm`.
    let rest = unsafe { libc::strptime(input.as_ptr(), pattern.as_ptr(), &mut t) };
    // SAFETY: a non-null result points into `input`, which is still alive.
    if rest.is_null() || !unsafe { CStr::from_ptr(rest) }.to_bytes().is_empty() {
        return Err(mismatch());
    }
    // `timegm` fills in the weekday and day of the year.
    Ok(parts(&to_tm(utc_seconds(t), false)?, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_report_character_offsets() {
        let found = match_impl(&[
            Value::str("aééb"),
            Value::str("é+"),
            Value::Null,
            Value::Bool(false),
        ])
        .unwrap();
        assert!(dump(&found).contains("\"offset\":1,\"length\":2"));
    }

    #[test]
    fn epoch_seconds_round_trip_through_broken_down_time() {
        let parts = broken_down(&Value::num(1425599621.0), false).unwrap();
        assert_eq!(dump(&parts), "[2015,2,5,23,53,41,4,63]");
        assert_eq!(dump(&mktime(&[parts]).unwrap()), "1425599621");
    }
}
