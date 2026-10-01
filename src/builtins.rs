//! Every builtin, defined once. Each has a name and an arity, and is one of:
//! a plain function of values, a special that controls evaluation itself, or a
//! definition written in the language, parsed only when a program calls it.

use std::rc::Rc;
use std::sync::LazyLock;

use crate::eval::{Env, Item, Out, Program, R, Stop, error, lift};
use crate::library;
use crate::parser::Node;
use crate::value::{self, Res, Value, compare, describe, type_error};
use crate::writer::dump;

/// A builtin over values: gets the input and then the evaluated arguments.
pub type Plain = fn(&[Value]) -> Res<Value>;
pub type Table = &'static [(&'static str, usize, Plain)];

pub enum How {
    Plain(Plain),
    Special(Special),
    /// The text of a `def`.
    Source(&'static str),
}

pub struct Builtin {
    pub name: &'static str,
    pub arity: usize,
    pub how: How,
}

static REGISTRY: LazyLock<Vec<Builtin>> = LazyLock::new(|| {
    let specials = SPECIALS.iter().map(|&(name, arity, special)| Builtin {
        name,
        arity,
        how: How::Special(special),
    });
    let families = [
        PLAIN,
        library::STRINGS,
        library::DATES,
        library::UNARY,
        library::BINARY,
        library::OTHER,
    ];
    let plains = families
        .into_iter()
        .flatten()
        .map(|&(name, arity, function)| Builtin {
            name,
            arity,
            how: How::Plain(function),
        });
    let sources = SOURCES.iter().map(|&text| {
        let head = &text["def ".len()..];
        let end = head.find([':', '(']).unwrap_or(head.len());
        let params = head[end..].strip_prefix('(').map_or(0, |p| {
            p[..p.find(')').unwrap_or(p.len())].split(';').count()
        });
        Builtin {
            name: head[..end].trim(),
            arity: params,
            how: How::Source(text),
        }
    });
    specials.chain(plains).chain(sources).collect()
});

pub fn registry() -> &'static [Builtin] {
    &REGISTRY
}

/// The position in the registry of the builtin with this name and arity.
pub fn resolve(name: &str, arity: usize) -> Option<usize> {
    registry()
        .iter()
        .position(|b| b.name == name && b.arity == arity)
}

/// A builtin that receives its arguments as unevaluated filters, or reaches into
/// the interpreter's inputs and environment.
pub type Special = fn(&Program, &[Node], &Env, Item, Out<'_>) -> R;

const SPECIALS: &[(&str, usize, Special)] = &[
    ("empty", 0, empty),
    ("error", 0, raise),
    ("select", 1, select),
    ("path", 1, path),
    ("getpath", 1, getpath),
    ("range", 1, range),
    ("range", 2, range),
    ("range", 3, range),
    ("map", 1, map),
    ("input", 0, input),
    ("inputs", 0, inputs),
    ("debug", 0, debug),
    ("halt_error", 1, halt_error),
    ("env", 0, env),
    ("builtins", 0, builtins),
    ("input_filename", 0, input_filename),
    ("input_line_number", 0, input_line_number),
];

fn empty(_: &Program, _: &[Node], _: &Env, _: Item, _: Out<'_>) -> R {
    Ok(())
}

fn raise(_: &Program, _: &[Node], _: &Env, input: Item, _: Out<'_>) -> R {
    Err(Stop::Error(input.value))
}

fn select(interp: &Program, args: &[Node], env: &Env, input: Item, out: Out<'_>) -> R {
    let condition = input.detached();
    interp.eval(args[0], env, condition, &mut |c| {
        if c.value.truthy() {
            out(input.clone())
        } else {
            Ok(())
        }
    })
}

fn path(interp: &Program, args: &[Node], env: &Env, input: Item, out: Out<'_>) -> R {
    let paths = interp.paths(args[0], env, input.value.clone())?;
    paths
        .into_iter()
        .try_for_each(|keys| out(input.derive(Value::arr(keys))))
}

/// `getpath(p)`; in a path expression the result extends the path with `p`.
fn getpath(interp: &Program, args: &[Node], env: &Env, input: Item, out: Out<'_>) -> R {
    let wanted = input.detached();
    interp.eval(args[0], env, wanted, &mut |p| {
        let Value::Arr(keys) = &p.value else {
            return Err(error("Path must be specified as an array"));
        };
        let value = lift(value::get_path(&input.value, keys))?;
        let found = Item {
            value,
            track: input.track.clone(),
        };
        out(keys.iter().fold(found, |item, key| Item {
            track: item.track.push(key.clone()),
            ..item
        }))
    })
}

/// `range(upto)`, `range(from; upto)` and `range(from; upto; by)`.
fn range(interp: &Program, args: &[Node], env: &Env, input: Item, out: Out<'_>) -> R {
    // Reversed, so that the first argument varies slowest.
    let reversed: Vec<Node> = args.iter().rev().copied().collect();
    interp.combinations(&reversed, env, &input.value, &mut |values| {
        let numbers: Option<Vec<f64>> = values.iter().rev().map(Value::as_f64).collect();
        let (mut at, upto, by) =
            match numbers.ok_or_else(|| error("Range bounds must be numeric"))?[..] {
                [upto] => (0.0, upto, 1.0),
                [from, upto] => (from, upto, 1.0),
                [from, upto, by] => (from, upto, by),
                _ => unreachable!("range takes one to three arguments"),
            };
        while (by > 0.0 && at < upto) || (by < 0.0 && at > upto) {
            out(input.derive(Value::num(at)))?;
            at += by;
        }
        Ok(())
    })
}

/// `map(f)`, which is `[.[] | f]`.
fn map(interp: &Program, args: &[Node], env: &Env, input: Item, out: Out<'_>) -> R {
    let items: Vec<Value> = match &input.value {
        Value::Arr(items) => items.to_vec(),
        Value::Obj(map) => map.values().cloned().collect(),
        other => return Err(error(format!("Cannot iterate over {}", describe(other)))),
    };
    let mut mapped = Vec::with_capacity(items.len());
    for item in items {
        interp.eval(args[0], env, Item::plain(item), &mut |output| {
            mapped.push(output.value);
            Ok(())
        })?;
    }
    out(input.derive(Value::arr(mapped)))
}

fn input(interp: &Program, _: &[Node], _: &Env, input: Item, out: Out<'_>) -> R {
    match interp.next_input() {
        Some(Ok(value)) => out(input.derive(value)),
        Some(Err(message)) => Err(error(message)),
        None => Err(error("No more inputs")),
    }
}

fn inputs(interp: &Program, _: &[Node], _: &Env, input: Item, out: Out<'_>) -> R {
    while let Some(next) = interp.next_input() {
        out(input.derive(next.map_err(error)?))?;
    }
    Ok(())
}

fn debug(_: &Program, _: &[Node], _: &Env, input: Item, out: Out<'_>) -> R {
    let message = Value::arr(vec![Value::str("DEBUG:"), input.value.clone()]);
    tracing::warn!("{}", dump(&message));
    out(input)
}

/// `halt_error(code)`: prints the input (strings raw) to stderr and exits.
fn halt_error(interp: &Program, args: &[Node], env: &Env, input: Item, _: Out<'_>) -> R {
    let code = input.detached();
    interp.eval(args[0], env, code, &mut |code| {
        let Some(code) = code.value.as_f64() else {
            return Err(error("halt_error/1: number required"));
        };
        let message = match &input.value {
            Value::Str(s) => s.to_string(),
            other => format!("{}\n", dump(other)),
        };
        Err(Stop::Halt {
            code: code as i32,
            message: Some(message),
        })
    })
}

fn env(interp: &Program, _: &[Node], _: &Env, input: Item, out: Out<'_>) -> R {
    out(input.derive(interp.environment()))
}

fn builtins(_: &Program, _: &[Node], _: &Env, input: Item, out: Out<'_>) -> R {
    let names = registry()
        .iter()
        .filter(|b| !b.name.starts_with('_'))
        .map(|b| Value::str(format!("{}/{}", b.name, b.arity)))
        .collect();
    out(input.derive(Value::arr(names)))
}

fn input_filename(interp: &Program, _: &[Node], _: &Env, input: Item, out: Out<'_>) -> R {
    let name = interp
        .inputs
        .borrow()
        .as_ref()
        .and_then(|records| records.filename());
    out(input.derive(name.map_or(Value::Null, Value::str)))
}

fn input_line_number(interp: &Program, _: &[Node], _: &Env, input: Item, out: Out<'_>) -> R {
    let line = interp
        .inputs
        .borrow()
        .as_ref()
        .map_or(0, |records| records.line());
    out(input.derive(Value::num(line as f64)))
}

const PLAIN: Table = &[
    ("not", 0, not),
    ("type", 0, type_of),
    ("length", 0, length),
    ("keys_unsorted", 0, keys_unsorted),
    ("has", 1, has),
    ("contains", 1, contains_value),
    ("add", 0, add_all),
    ("flatten", 0, flatten),
    ("flatten", 1, flatten),
    ("sort", 0, sort),
    ("unique", 0, unique),
    ("_sort_by_impl", 1, sort_by),
    ("_group_by_impl", 1, group_by),
    ("bsearch", 1, bsearch),
    ("setpath", 2, setpath),
    ("delpaths", 1, delpaths),
    ("have_literal_numbers", 0, |_| Ok(Value::Bool(true))),
    ("have_decnum", 0, |_| Ok(Value::Bool(false))),
];

fn not(v: &[Value]) -> Res<Value> {
    Ok(Value::Bool(!v[0].truthy()))
}

fn type_of(v: &[Value]) -> Res<Value> {
    Ok(Value::str(v[0].kind()))
}

fn length(v: &[Value]) -> Res<Value> {
    Ok(Value::num(match &v[0] {
        Value::Null => 0.0,
        Value::Num(n, _) => n.abs(),
        Value::Str(s) => s.chars().count() as f64,
        Value::Arr(items) => items.len() as f64,
        Value::Obj(map) => map.len() as f64,
        other => return Err(type_error(other, "has no length")),
    }))
}

fn keys_unsorted(v: &[Value]) -> Res<Value> {
    match &v[0] {
        Value::Obj(map) => Ok(Value::arr(
            map.keys().map(|k| Value::Str(k.clone())).collect(),
        )),
        Value::Arr(items) => Ok(Value::arr(
            (0..items.len()).map(|i| Value::num(i as f64)).collect(),
        )),
        other => Err(type_error(other, "has no keys")),
    }
}

fn has(v: &[Value]) -> Res<Value> {
    match (&v[0], &v[1]) {
        (Value::Obj(map), Value::Str(key)) => Ok(Value::Bool(map.contains_key(&key[..]))),
        (Value::Arr(items), Value::Num(n, _)) => {
            Ok(Value::Bool(*n >= 0.0 && *n < items.len() as f64))
        }
        (a, b) => Err(format!(
            "Cannot check whether {} has a {} key",
            a.kind(),
            b.kind()
        )),
    }
}

fn contains_value(v: &[Value]) -> Res<Value> {
    value::contains(&v[0], &v[1]).map(Value::Bool)
}

fn items<'a>(value: &'a Value, action: &str) -> Res<&'a Rc<Vec<Value>>> {
    match value {
        Value::Arr(items) => Ok(items),
        other => Err(type_error(other, action)),
    }
}

fn add_all(v: &[Value]) -> Res<Value> {
    let values: Vec<Value> = match &v[0] {
        Value::Null => return Ok(Value::Null),
        Value::Arr(items) => items.to_vec(),
        Value::Obj(map) => map.values().cloned().collect(),
        other => return Err(format!("Cannot iterate over {}", describe(other))),
    };
    values.into_iter().try_fold(Value::Null, value::add)
}

fn flatten(v: &[Value]) -> Res<Value> {
    let depth = match v.get(1) {
        None => 1e9,
        Some(Value::Num(n, _)) if *n >= 0.0 => *n,
        Some(Value::Num(..)) => return Err("flatten depth must not be negative".into()),
        Some(other) => return Err(type_error(other, "is not a number")),
    };
    fn spread(items: &[Value], depth: f64, into: &mut Vec<Value>) {
        for item in items {
            match item {
                Value::Arr(inner) if depth > 0.0 => spread(inner, depth - 1.0, into),
                other => into.push(other.clone()),
            }
        }
    }
    let mut flat = Vec::new();
    spread(items(&v[0], "cannot be flattened")?, depth, &mut flat);
    Ok(Value::arr(flat))
}

fn sorted_values(value: &Value) -> Res<Vec<Value>> {
    let mut values = items(value, "cannot be sorted, as it is not an array")?.to_vec();
    values.sort_by(compare);
    Ok(values)
}

fn sort(v: &[Value]) -> Res<Value> {
    sorted_values(&v[0]).map(Value::arr)
}

fn unique(v: &[Value]) -> Res<Value> {
    let mut values = sorted_values(&v[0])?;
    values.dedup_by(|a, b| compare(a, b).is_eq());
    Ok(Value::arr(values))
}

/// The elements of `v[0]` with the sort key `v[1]` computed for each, in key order.
fn keyed(v: &[Value], action: &str) -> Res<Vec<(Value, Value)>> {
    let values = items(&v[0], action)?;
    let keys = items(&v[1], action)?;
    let mut pairs: Vec<_> = keys.iter().cloned().zip(values.iter().cloned()).collect();
    pairs.sort_by(|a, b| compare(&a.0, &b.0));
    Ok(pairs)
}

fn sort_by(v: &[Value]) -> Res<Value> {
    let pairs = keyed(v, "cannot be sorted, as it is not an array")?;
    Ok(Value::arr(
        pairs.into_iter().map(|(_, value)| value).collect(),
    ))
}

fn group_by(v: &[Value]) -> Res<Value> {
    let mut groups: Vec<(Value, Vec<Value>)> = Vec::new();
    for (key, value) in keyed(v, "cannot be grouped, as it is not an array")? {
        match groups.last_mut() {
            Some((last, members)) if compare(last, &key).is_eq() => members.push(value),
            _ => groups.push((key, vec![value])),
        }
    }
    Ok(Value::arr(
        groups
            .into_iter()
            .map(|(_, members)| Value::arr(members))
            .collect(),
    ))
}

fn bsearch(v: &[Value]) -> Res<Value> {
    let values = items(&v[0], "cannot be searched from")?;
    Ok(Value::num(
        match values.binary_search_by(|probe| compare(probe, &v[1])) {
            Ok(found) => found as f64,
            Err(insert) => -1.0 - insert as f64,
        },
    ))
}

fn path_keys(value: &Value) -> Res<&[Value]> {
    match value {
        Value::Arr(keys) => Ok(keys),
        _ => Err("Path must be specified as an array".into()),
    }
}

fn setpath(v: &[Value]) -> Res<Value> {
    value::set_path(v[0].clone(), path_keys(&v[1])?, v[2].clone())
}

fn delpaths(v: &[Value]) -> Res<Value> {
    let Value::Arr(paths) = &v[1] else {
        return Err("Paths must be specified as an array".into());
    };
    let paths = paths
        .iter()
        .map(|p| path_keys(p).map(<[Value]>::to_vec))
        .collect::<Res<_>>()?;
    value::delete_paths(v[0].clone(), paths)
}

/// Builtins written in the language itself, one definition each. They are
/// parsed only when a program calls them.
const SOURCES: &[&str] = &[
    "def first(f): label $out | (f | ., break $out);",
    "def limit($n; f): if $n > 0 then label $out | foreach f as $item (0; . + 1; $item, if . >= $n then empty, break $out else empty end) elif $n == 0 then empty else f end;",
    "def recurse: recurse(.[]?);",
    "def recurse(f): def r: ., (f | r); r;",
    r#"def error(msg): msg | error;"#,
    r#"def halt_error: halt_error(5);"#,
    r#"def sort_by(f): _sort_by_impl(map([f]));"#,
    r#"def group_by(f): _group_by_impl(map([f]));"#,
    r#"def unique_by(f): [group_by(f)[] | .[0]];"#,
    r#"def keys: keys_unsorted | sort;"#,
    r#"def min: sort | .[0];"#,
    r#"def max: sort | .[-1];"#,
    r#"def reverse: [.[length - 1 - range(0; length)]];"#,
    r#"def min_by(f): sort_by(f) | .[0];"#,
    r#"def max_by(f): sort_by(f) | .[-1];"#,
    r#"def add(f): reduce f as $x (null; . + $x);"#,
    r#"def del(f): delpaths([path(f)]);"#,
    r#"def map_values(f): .[] |= f;"#,
    r#"def abs: if . < 0 then -. else . end;"#,
    r#"def recurse(f; cond): def step: ., (f | select(cond) | step); step;"#,
    r#"def to_entries: [keys_unsorted[] as $k | {key: $k, value: .[$k]}];"#,
    r#"def from_entries:
  reduce .[] as $e ({};
    ($e | if .key == null then .k // .name // .Name // .K // .Key else .key end) as $key
    | if $key == null then error("Cannot use null (null) as object key") else . end
    | . + {(if ($key | type) == "string" then $key else ($key | tojson) end):
           ($e | if has("value") then .value elif has("v") then .v else .Value // .V end)});"#,
    r#"def with_entries(f): to_entries | map(f) | from_entries;"#,
    r#"def indices($x):
  if type == "array" and ($x | type) == "array" then .[$x]
  elif type == "array" then .[[$x]]
  elif type == "string" and ($x | type) == "string" then _strindices($x)
  else .[$x] end;"#,
    r#"def index($x): indices($x) | .[0];"#,
    r#"def rindex($x): indices($x) | .[-1:][0];"#,
    r#"def paths: path(..) | select(length > 0);"#,
    r#"def paths(f): . as $in | paths | select(. as $p | $in | getpath($p) | f);"#,
    r#"def in(xs): . as $x | xs | has($x);"#,
    r#"def inside(xs): . as $x | xs | contains($x);"#,
    r#"def arrays: select(type == "array");"#,
    r#"def objects: select(type == "object");"#,
    r#"def iterables: select(type == "array" or type == "object");"#,
    r#"def booleans: select(type == "boolean");"#,
    r#"def numbers: select(type == "number");"#,
    r#"def strings: select(type == "string");"#,
    r#"def nulls: select(. == null);"#,
    r#"def values: select(. != null);"#,
    r#"def scalars: select(type != "array" and type != "object");"#,
    r#"def isfinite: type == "number" and (isinfinite | not);"#,
    r#"def finites: select(isfinite);"#,
    r#"def normals: select(isnormal);"#,
    r#"def fromdateiso8601: strptime("%Y-%m-%dT%H:%M:%SZ") | mktime;"#,
    r#"def todateiso8601: strftime("%Y-%m-%dT%H:%M:%SZ");"#,
    r#"def fromdate: fromdateiso8601;"#,
    r#"def todate: todateiso8601;"#,
    r#"def match(re; flags): _match_impl(re; flags; false) | .[];"#,
    r#"def match($spec):
  ($spec | type) as $kind
  | if $kind == "string" then match($spec; null)
    elif $kind == "array" and ($spec | length) > 0 then match($spec[0]; $spec[1])
    else error($kind + " not a string or array") end;"#,
    r#"def test(re; flags): _match_impl(re; flags; true);"#,
    r#"def test($spec):
  ($spec | type) as $kind
  | if $kind == "string" then test($spec; null)
    elif $kind == "array" and ($spec | length) > 0 then test($spec[0]; $spec[1])
    else error($kind + " not a string or array") end;"#,
    r#"def capture(re; flags):
  match(re; flags) | [.captures[] | select(.name != null) | {key: .name, value: .string}] | from_entries;"#,
    r#"def capture($spec):
  ($spec | type) as $kind
  | if $kind == "string" then capture($spec; null)
    elif $kind == "array" and ($spec | length) > 0 then capture($spec[0]; $spec[1])
    else error($kind + " not a string or array") end;"#,
    r#"def scan($re; $flags):
  match($re; "g" + $flags)
  | if (.captures | length) > 0 then [.captures[].string] else .string end;"#,
    r#"def scan($re): scan($re; null);"#,
    r#"def splits($re; $flags):
  . as $text
  | foreach ((match($re; $flags + "g") | [.offset, .length]), null) as $m
      ({from: 0, piece: null};
       if $m == null then .piece = $text[.from:]
       else .piece = $text[.from:$m[0]] | .from = $m[0] + $m[1] end;
       .piece);"#,
    r#"def splits($re): splits($re; null);"#,
    r#"def split($re; $flags): [splits($re; $flags)];"#,
    r#"def sub($re; replacement; $flags):
  . as $text
  | [match($re; $flags)] as $found
  | if $found == [] then $text
    else
      (reduce range(0; $found | length) as $i ({results: [], from: 0};
         $found[$i] as $m
         | $text[.from:$m.offset] as $gap
         | [($m.captures | map(select(.name != null) | {key: .name, value: .string}) | from_entries)
            | replacement] as $pieces
         | reduce range(0; $pieces | length) as $j (.; .results[$j] += $gap + $pieces[$j])
         | .from = $m.offset + $m.length)
       | .from as $end
       | .results[] + $text[$end:]) // $text
    end;"#,
    r#"def sub($re; replacement): sub($re; replacement; "");"#,
    r#"def gsub($re; replacement; flags): sub($re; replacement; flags + "g");"#,
    r#"def gsub($re; replacement): sub($re; replacement; "g");"#,
    r#"def ltrimstr($prefix):
  if type == "string" and ($prefix | type) == "string" and startswith($prefix)
  then .[($prefix | length):] else . end;"#,
    r#"def rtrimstr($suffix):
  if type == "string" and ($suffix | type) == "string" and endswith($suffix)
  then .[:length - ($suffix | length)] else . end;"#,
    r#"def trimstr($x): ltrimstr($x) | rtrimstr($x);"#,
    r#"def while(cond; update): def go: if cond then ., (update | go) else empty end; go;"#,
    r#"def until(cond; step): def go: if cond then . else (step | go) end; go;"#,
    r#"def repeat(f): def go: f, go; go;"#,
    r#"def skip($n; expr):
  if $n < 0 then error("skip doesn't support negative count")
  else foreach expr as $item ($n; . - 1; if . < 0 then $item else empty end) end;"#,
    r#"def isempty(g): first((g | false), true);"#,
    r#"def all(gen; cond): isempty(gen | select(cond | not));"#,
    r#"def any(gen; cond): isempty(gen | select(cond)) | not;"#,
    r#"def all(cond): all(.[]; cond);"#,
    r#"def any(cond): any(.[]; cond);"#,
    r#"def all: all(.[]; .);"#,
    r#"def any: any(.[]; .);"#,
    r#"def nth($n; g):
  if $n < 0 then error("nth doesn't support negative indices") else first(skip($n; g)) end;"#,
    r#"def first: .[0];"#,
    r#"def last: .[-1];"#,
    r#"def last(f): reduce f as $x (null; $x);"#,
    r#"def nth($n): .[$n];"#,
    r#"def combinations:
  if length == 0 then []
  else .[0][] as $head | (.[1:] | combinations) as $tail | [$head] + $tail end;"#,
    r#"def combinations(n): . as $v | [range(n) | $v] | combinations;"#,
    r#"def transpose: (map(length) | max // 0) as $width | [range(0; $width) as $i | map(.[$i])];"#,
    r#"def tostream:
  path(def post: (.[]? | post), .; post) as $p
  | getpath($p) as $v
  | if ($v | type) == "array" and ($v | length) > 0 then [$p + [$v | length - 1]]
    elif ($v | type) == "object" and ($v | length) > 0 then [$p + [$v | keys_unsorted | last]]
    else [$p, $v] end;"#,
    r#"def fromstream(events):
  {value: null, done: false} as $fresh
  | foreach events as $event ($fresh;
      (if .done then $fresh else . end)
      | if ($event | length) == 2
        then .value |= setpath($event[0]; $event[1]) | .done = ($event[0] | length == 0)
        else .done = ($event[0] | length == 1) end;
      if .done then .value else empty end);"#,
    r#"def truncate_stream(stream):
  . as $depth | null | stream | select((.[0] | length) > $depth) | .[0] |= .[$depth:];"#,
    r#"def walk(f): def go: (if type == "object" then map_values(go) elif type == "array" then map(go) else . end) | f; go;"#,
    r#"def pick(selection): . as $in | reduce path(selection) as $p (null; setpath($p; $in | getpath($p)));"#,
    r#"def debug(msgs): (msgs | debug | empty), .;"#,
    r#"def INDEX(stream; key): reduce stream as $row ({}; .[$row | key | tostring] = $row);"#,
    r#"def INDEX(key): INDEX(.[]; key);"#,
    r#"def JOIN($index; key): [.[] | [., $index[key]]];"#,
    r#"def JOIN($index; stream; key): stream | [., $index[key]];"#,
    r#"def JOIN($index; stream; key; shape): JOIN($index; stream; key) | shape;"#,
    r#"def IN(s): any(s == .; .);"#,
    r#"def IN(src; s): any(src == s; .);"#,
];
