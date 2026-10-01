//! The value model, and everything done with values: ordering, arithmetic,
//! indexing, slicing, and path access. Cloning is cheap: strings, arrays and
//! objects are shared. Failures are plain message strings.

use std::cmp::Ordering;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::writer::dump;

pub type Map = IndexMap<Rc<str>, Value>;

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    /// A double, plus the canonical text it was read from when that text is
    /// not what the double prints as (`1.50`, `1E+2`, big integers).
    Num(f64, Option<Rc<str>>),
    Str(Rc<str>),
    Arr(Rc<Vec<Value>>),
    Obj(Rc<Map>),
}

impl Value {
    pub fn num(n: f64) -> Self {
        Self::Num(n, None)
    }

    pub fn str(s: impl AsRef<str>) -> Self {
        Self::Str(s.as_ref().into())
    }

    pub fn arr(items: Vec<Value>) -> Self {
        Self::Arr(Rc::new(items))
    }

    pub fn obj(map: Map) -> Self {
        Self::Obj(Rc::new(map))
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Num(..) => "number",
            Self::Str(_) => "string",
            Self::Arr(_) => "array",
            Self::Obj(_) => "object",
        }
    }

    pub fn truthy(&self) -> bool {
        !matches!(self, Self::Null | Self::Bool(false))
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Num(n, _) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn rank(&self) -> u8 {
        match self {
            Self::Null => 0,
            Self::Bool(false) => 1,
            Self::Bool(true) => 2,
            Self::Num(..) => 3,
            Self::Str(_) => 4,
            Self::Arr(_) => 5,
            Self::Obj(_) => 6,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        compare(self, other) == Ordering::Equal
    }
}

/// The total order: null < false < true < numbers < strings < arrays < objects.
/// NaN sorts below every number, itself included.
pub fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Num(x, _), Value::Num(y, _)) => {
            if x.is_nan() {
                Ordering::Less
            } else if y.is_nan() {
                Ordering::Greater
            } else {
                x.partial_cmp(y).unwrap_or(Ordering::Equal)
            }
        }
        (Value::Str(x), Value::Str(y)) => x.as_bytes().cmp(y.as_bytes()),
        (Value::Arr(x), Value::Arr(y)) => compare_lists(x, y),
        (Value::Obj(x), Value::Obj(y)) => {
            let (x, y) = (sorted(x), sorted(y));
            let keys = x
                .iter()
                .map(|e| e.0.as_bytes())
                .cmp(y.iter().map(|e| e.0.as_bytes()));
            keys.then_with(|| {
                x.iter()
                    .zip(&y)
                    .map(|(x, y)| compare(x.1, y.1))
                    .find(|order| order.is_ne())
                    .unwrap_or(Ordering::Equal)
            })
        }
        _ => a.rank().cmp(&b.rank()),
    }
}

fn compare_lists(a: &[Value], b: &[Value]) -> Ordering {
    a.iter()
        .zip(b)
        .map(|(x, y)| compare(x, y))
        .find(|order| order.is_ne())
        .unwrap_or_else(|| a.len().cmp(&b.len()))
}

/// Entries ordered by key, as `keys`, `-S` and comparisons use.
pub fn sorted(map: &Map) -> Vec<(&Rc<str>, &Value)> {
    let mut entries: Vec<_> = map.iter().collect();
    entries.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    entries
}

pub type Res<T> = Result<T, String>;

/// `text` cut to at most `max` bytes, ending in `...` when it was cut.
pub fn truncate(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let mut end = max - 3;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// `kind (dump)`, with the dump cut to fit an error message.
pub fn describe(value: &Value) -> String {
    format!("{} ({})", value.kind(), truncate(dump(value), 14))
}

pub fn type_error(value: &Value, message: &str) -> String {
    format!("{} {message}", describe(value))
}

pub fn type_error2(a: &Value, b: &Value, message: &str) -> String {
    format!("{} and {} {message}", describe(a), describe(b))
}

pub fn add(a: Value, b: Value) -> Res<Value> {
    Ok(match (a, b) {
        (Value::Null, b) => b,
        (a, Value::Null) => a,
        (Value::Num(a, _), Value::Num(b, _)) => Value::num(a + b),
        (Value::Str(a), Value::Str(b)) => Value::str([&*a, &*b].concat()),
        (Value::Arr(mut a), Value::Arr(b)) => {
            Rc::make_mut(&mut a).extend(b.iter().cloned());
            Value::Arr(a)
        }
        (Value::Obj(mut a), Value::Obj(b)) => {
            let merged = Rc::make_mut(&mut a);
            merged.extend(b.iter().map(|(k, v)| (k.clone(), v.clone())));
            Value::Obj(a)
        }
        (a, b) => return Err(type_error2(&a, &b, "cannot be added")),
    })
}

pub fn subtract(a: Value, b: Value) -> Res<Value> {
    Ok(match (&a, &b) {
        (Value::Num(x, _), Value::Num(y, _)) => Value::num(x - y),
        (Value::Arr(x), Value::Arr(y)) => {
            Value::arr(x.iter().filter(|item| !y.contains(item)).cloned().collect())
        }
        _ => return Err(type_error2(&a, &b, "cannot be subtracted")),
    })
}

pub fn multiply(a: Value, b: Value) -> Res<Value> {
    Ok(match (a, b) {
        (Value::Num(a, _), Value::Num(b, _)) => Value::num(a * b),
        (Value::Str(s), Value::Num(n, _)) | (Value::Num(n, _), Value::Str(s)) => {
            if n.is_nan() || n < 0.0 {
                Value::Null
            } else {
                Value::str(s.repeat(n as usize))
            }
        }
        (Value::Obj(a), Value::Obj(b)) => Value::Obj(merge_deep(a, &b)),
        (a, b) => return Err(type_error2(&a, &b, "cannot be multiplied")),
    })
}

fn merge_deep(mut target: Rc<Map>, source: &Map) -> Rc<Map> {
    let merged = Rc::make_mut(&mut target);
    for (key, value) in source {
        let combined = match (merged.get(key), value) {
            (Some(Value::Obj(existing)), Value::Obj(incoming)) => {
                Value::Obj(merge_deep(existing.clone(), incoming))
            }
            _ => value.clone(),
        };
        merged.insert(key.clone(), combined);
    }
    target
}

pub fn divide(a: Value, b: Value) -> Res<Value> {
    Ok(match (&a, &b) {
        (Value::Num(x, _), Value::Num(y, _)) => {
            if *y == 0.0 {
                return Err(type_error2(
                    &a,
                    &b,
                    "cannot be divided because the divisor is zero",
                ));
            }
            Value::num(x / y)
        }
        (Value::Str(x), Value::Str(y)) => Value::arr(split(x, y)),
        _ => return Err(type_error2(&a, &b, "cannot be divided")),
    })
}

pub fn split(text: &str, separator: &str) -> Vec<Value> {
    if text.is_empty() {
        return Vec::new();
    }
    if separator.is_empty() {
        return text.chars().map(|c| Value::str(c.to_string())).collect();
    }
    text.split(separator).map(Value::str).collect()
}

pub fn remainder(a: Value, b: Value) -> Res<Value> {
    match (&a, &b) {
        (Value::Num(x, _), Value::Num(y, _)) if x.is_nan() || y.is_nan() => {
            Ok(Value::num(f64::NAN))
        }
        (Value::Num(x, _), Value::Num(y, _)) => {
            let (x, y) = (*x as i64, *y as i64);
            if y == 0 {
                return Err(type_error2(
                    &a,
                    &b,
                    "cannot be divided (remainder) because the divisor is zero",
                ));
            }
            Ok(Value::num(x.wrapping_rem(y) as f64))
        }
        _ => Err(type_error2(&a, &b, "cannot be divided")),
    }
}

pub fn negate(value: Value) -> Res<Value> {
    match value {
        Value::Num(n, _) => Ok(Value::num(-n)),
        other => Err(type_error(&other, "cannot be negated")),
    }
}

/// Resolves slice bounds against a length the usual way.
fn slice_bounds(len: usize, from: &Value, to: &Value) -> Res<(usize, usize)> {
    let bound = |value: &Value, default: f64| match value {
        Value::Null => Ok(default),
        Value::Num(n, _) if n.is_nan() => Ok(default),
        Value::Num(n, _) => Ok(*n),
        _ => Err("Start and end indices of an array slice must be numbers".to_string()),
    };
    let len_f = len as f64;
    let (mut start, mut end) = (bound(from, 0.0)?, bound(to, len_f)?);
    if start < 0.0 {
        start += len_f;
    }
    if end < 0.0 {
        end += len_f;
    }
    start = start.clamp(0.0, len_f);
    end = end.clamp(start, len_f);
    Ok((start as usize, end.ceil() as usize))
}

pub fn slice(value: &Value, from: &Value, to: &Value) -> Res<Value> {
    match value {
        Value::Null => Ok(Value::Null),
        Value::Arr(items) => {
            let (start, end) = slice_bounds(items.len(), from, to)?;
            Ok(Value::arr(items[start..end].to_vec()))
        }
        Value::Str(text) => {
            let (start, end) = slice_bounds(text.chars().count(), from, to)?;
            let byte = |n: usize| text.char_indices().nth(n).map_or(text.len(), |(i, _)| i);
            Ok(Value::str(&text[byte(start)..byte(end)]))
        }
        other => Err(format!("Cannot index {} with object", other.kind())),
    }
}

/// The `{start, end}` object that indexes a slice.
pub fn slice_key(start: Value, end: Value) -> Value {
    Value::obj(Map::from_iter([
        ("start".into(), start),
        ("end".into(), end),
    ]))
}

fn slice_ends(key: &Map) -> (&Value, &Value) {
    (
        key.get("start").unwrap_or(&Value::Null),
        key.get("end").unwrap_or(&Value::Null),
    )
}

fn cannot_index(value: &Value, key: &Value) -> String {
    match key {
        Value::Str(k) => format!("Cannot index {} with string \"{k}\"", value.kind()),
        _ => format!("Cannot index {} with {}", value.kind(), key.kind()),
    }
}

/// An array index, counted from the end when negative.
fn from_end(len: usize, n: f64) -> f64 {
    let index = n.trunc();
    if index < 0.0 {
        index + len as f64
    } else {
        index
    }
}

fn array_position(len: usize, n: f64) -> Option<usize> {
    let index = from_end(len, n);
    (index >= 0.0 && index < len as f64).then_some(index as usize)
}

pub fn index(value: &Value, key: &Value) -> Res<Value> {
    match (value, key) {
        (Value::Obj(map), Value::Str(k)) => Ok(map.get(k).cloned().unwrap_or_default()),
        (Value::Arr(items), Value::Num(n, _)) => {
            Ok(array_position(items.len(), *n).map_or(Value::Null, |i| items[i].clone()))
        }
        (Value::Arr(_) | Value::Str(_) | Value::Null, Value::Obj(bounds)) => {
            let (from, to) = slice_ends(bounds);
            slice(value, from, to)
        }
        (Value::Arr(items), Value::Arr(needle)) => Ok(subarray_positions(items, needle)),
        (Value::Null, Value::Str(_) | Value::Num(..) | Value::Null) => Ok(Value::Null),
        _ => Err(cannot_index(value, key)),
    }
}

pub fn subarray_positions(items: &[Value], needle: &[Value]) -> Value {
    if needle.is_empty() || needle.len() > items.len() {
        return Value::arr(Vec::new());
    }
    let positions = items
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| window == &needle)
        .map(|(i, _)| Value::num(i as f64))
        .collect();
    Value::arr(positions)
}

pub fn get_path(root: &Value, path: &[Value]) -> Res<Value> {
    let mut current = root.clone();
    for key in path {
        if matches!(current, Value::Null) {
            break;
        }
        current = index(&current, key)?;
    }
    Ok(current)
}

pub fn set_path(mut root: Value, path: &[Value], new: Value) -> Res<Value> {
    let Some((key, rest)) = path.split_first() else {
        return Ok(new);
    };
    if matches!(root, Value::Null) {
        match key {
            Value::Str(_) => root = Value::obj(Map::new()),
            Value::Num(..) | Value::Obj(_) => root = Value::arr(Vec::new()),
            _ => {}
        }
    }
    match (&mut root, key) {
        (Value::Obj(map), Value::Str(k)) => {
            let map = Rc::make_mut(map);
            let child = map.get_mut(k).map(std::mem::take).unwrap_or_default();
            map.insert(k.clone(), set_path(child, rest, new)?);
        }
        (Value::Arr(_), Value::Num(n, _)) if n.is_nan() => {
            return Err("Cannot set array element at NaN index".into());
        }
        (Value::Str(_), Value::Obj(_)) => return Err("Cannot update string slices".into()),
        (Value::Arr(items), Value::Num(n, _)) => {
            let items = Rc::make_mut(items);
            let index = from_end(items.len(), *n);
            if index < 0.0 {
                return Err("Out of bounds negative array index".into());
            }
            let index = index as usize;
            if index >= items.len() {
                items.resize(index + 1, Value::Null);
            }
            let child = std::mem::take(&mut items[index]);
            items[index] = set_path(child, rest, new)?;
        }
        (Value::Arr(items), Value::Obj(bounds)) => {
            let (from, to) = slice_ends(bounds);
            let (start, end) = slice_bounds(items.len(), from, to)?;
            let part = Value::arr(items[start..end].to_vec());
            let Value::Arr(replacement) = set_path(part, rest, new)? else {
                return Err("A slice of an array can only be assigned another array".into());
            };
            Rc::make_mut(items).splice(start..end, replacement.iter().cloned());
        }
        (Value::Arr(_), _) => return Err("Cannot update field at array index of array".into()),
        _ => return Err(cannot_index(&root, key)),
    }
    Ok(root)
}

/// The path with negative indices and slice bounds resolved against the
/// containers they index, so deleting one path cannot shift another.
fn normalize(root: &Value, path: Vec<Value>) -> Vec<Value> {
    let mut current = root.clone();
    let mut resolved = Vec::with_capacity(path.len());
    for key in path {
        let key = match (&current, &key) {
            (Value::Arr(items), Value::Num(n, _)) if *n < 0.0 => Value::num(n + items.len() as f64),
            (Value::Arr(items), Value::Obj(bounds)) => {
                let (from, to) = slice_ends(bounds);
                match slice_bounds(items.len(), from, to) {
                    Ok((start, end)) => slice_key(Value::num(start as f64), Value::num(end as f64)),
                    Err(_) => key,
                }
            }
            _ => key,
        };
        current = index(&current, &key).unwrap_or_default();
        resolved.push(key);
    }
    resolved
}

pub fn delete_paths(mut root: Value, paths: Vec<Vec<Value>>) -> Res<Value> {
    let mut paths: Vec<_> = paths
        .into_iter()
        .map(|path| normalize(&root, path))
        .collect();
    paths.sort_by(|a, b| compare_lists(a, b));
    for path in paths.iter().rev() {
        root = delete_path(root, path)?;
    }
    Ok(root)
}

fn delete_path(mut root: Value, path: &[Value]) -> Res<Value> {
    let Some((key, rest)) = path.split_first() else {
        return Ok(Value::Null);
    };
    if matches!(root, Value::Null) {
        return Ok(root);
    }
    if !rest.is_empty() {
        let child = index(&root, key)?;
        if matches!(child, Value::Null) {
            return Ok(root);
        }
        return set_path(root, std::slice::from_ref(key), delete_path(child, rest)?);
    }
    match (&mut root, key) {
        (Value::Obj(map), Value::Str(k)) => {
            Rc::make_mut(map).shift_remove(&k[..]);
        }
        (Value::Arr(items), Value::Num(n, _)) => {
            if let Some(i) = array_position(items.len(), *n) {
                Rc::make_mut(items).remove(i);
            }
        }
        (Value::Arr(items), Value::Obj(bounds)) => {
            let (from, to) = slice_ends(bounds);
            let (start, end) = slice_bounds(items.len(), from, to)?;
            Rc::make_mut(items).drain(start..end);
        }
        (Value::Obj(_), _) => {
            return Err(format!(
                "Cannot delete field at object index of {}",
                key.kind()
            ));
        }
        (Value::Arr(_), _) => {
            return Err(format!(
                "Cannot delete field at array index of {}",
                key.kind()
            ));
        }
        _ => return Err(format!("Cannot delete field at index of {}", root.kind())),
    }
    Ok(root)
}

pub fn contains(a: &Value, b: &Value) -> Res<bool> {
    match (a, b) {
        (Value::Obj(x), Value::Obj(y)) => {
            for (key, wanted) in y.iter() {
                match x.get(key) {
                    Some(have) if contains(have, wanted)? => {}
                    _ => return Ok(false),
                }
            }
            Ok(true)
        }
        (Value::Arr(x), Value::Arr(y)) => Ok(y
            .iter()
            .all(|wanted| x.iter().any(|have| contains(have, wanted).unwrap_or(false)))),
        (Value::Str(x), Value::Str(y)) => Ok(x.contains(&y[..])),
        _ if a.kind() == b.kind() => Ok(compare(a, b) == Ordering::Equal),
        _ => Err(type_error2(a, b, "cannot have their containment checked")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_have_a_total_order() {
        let ordered = [
            Value::Null,
            Value::Bool(false),
            Value::Bool(true),
            Value::num(f64::NAN),
            Value::num(-1.0),
            Value::str("a"),
            Value::arr(vec![]),
            Value::obj(Map::new()),
        ];
        for pair in ordered.windows(2) {
            assert_eq!(compare(&pair[0], &pair[1]), Ordering::Less);
        }
        assert_ne!(Value::num(f64::NAN), Value::num(f64::NAN));
    }

    #[test]
    fn objects_compare_by_sorted_keys_then_values() {
        let a: Map = [("b".into(), Value::num(1.0)), ("a".into(), Value::num(2.0))]
            .into_iter()
            .collect();
        let b: Map = [("a".into(), Value::num(2.0)), ("b".into(), Value::num(1.0))]
            .into_iter()
            .collect();
        assert_eq!(Value::obj(a), Value::obj(b));
    }

    #[test]
    fn paths_are_created_and_deleted() {
        let set = set_path(
            Value::Null,
            &[Value::str("a"), Value::num(1.0)],
            Value::num(5.0),
        )
        .unwrap();
        assert_eq!(dump(&set), r#"{"a":[null,5]}"#);
        let deleted = delete_paths(set, vec![vec![Value::str("a"), Value::num(0.0)]]).unwrap();
        assert_eq!(dump(&deleted), r#"{"a":[5]}"#);
    }

    #[test]
    fn slices_clamp_to_the_length() {
        let items = Value::arr((0..5).map(|n| Value::num(f64::from(n))).collect());
        let part = slice(&items, &Value::num(-2.0), &Value::Null).unwrap();
        assert_eq!(dump(&part), "[3,4]");
    }
}
