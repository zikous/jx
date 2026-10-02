//! The evaluator. Filters are generators: evaluating one hands each output to
//! a callback, which keeps evaluation allocation-free and lets consumers stop
//! early. A second mode tracks paths, which `path(f)`, assignment and `del`
//! are built on.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use crate::builtins::{self, How};
use crate::error::{Error, Result as AppResult};
use crate::library;
use crate::parser::{self, *};
use crate::reader;
use crate::records::Records;
use crate::value::{self, Map, Value, compare, describe, truncate};
use crate::writer::dump;

const STACK_RED_ZONE: usize = 32 * 1024;
const STACK_GROWTH: usize = 2 * 1024 * 1024;

/// Why evaluation stopped early.
pub enum Stop {
    /// An error, carrying any value.
    Error(Value),
    /// `break $label`, with the label's unique id.
    Break(u64),
    /// `halt` and `halt_error`.
    Halt { code: i32, message: Option<String> },
    /// Internal: a consumer downstream of a `try` failed.
    Passthrough,
}

pub type R = Result<(), Stop>;
pub type Out<'s> = &'s mut dyn FnMut(Item) -> R;

pub fn error(message: impl Into<String>) -> Stop {
    Stop::Error(Value::str(message.into()))
}

pub fn lift<T>(result: value::Res<T>) -> Result<T, Stop> {
    result.map_err(error)
}

/// Whether an item is a path expression result.
#[derive(Clone)]
pub enum Track {
    /// Not tracking paths.
    Off,
    /// A valid path from the root.
    At(Vec<Value>),
    /// Tracking, but this value did not come from a path expression.
    Invalid,
}

impl Track {
    fn derived(&self) -> Self {
        match self {
            Self::Off => Self::Off,
            _ => Self::Invalid,
        }
    }

    pub fn push(&self, key: Value) -> Self {
        match self {
            Self::At(keys) => Self::At([keys.as_slice(), &[key]].concat()),
            other => other.clone(),
        }
    }
}

#[derive(Clone)]
pub struct Item {
    pub value: Value,
    pub track: Track,
}

impl Item {
    pub fn plain(value: Value) -> Self {
        Self {
            value,
            track: Track::Off,
        }
    }

    /// The same value, no longer tracking a path.
    pub fn detached(&self) -> Self {
        Self::plain(self.value.clone())
    }

    /// A value computed from this item rather than selected from it.
    pub fn derive(&self, value: Value) -> Self {
        Self {
            value,
            track: self.track.derived(),
        }
    }

    fn child(&self, value: Value, key: Value) -> Self {
        Self {
            value,
            track: self.track.push(key),
        }
    }
}

enum Binding {
    Var(Name, Value),
    Closure(Name, Node, Env),
    Func(&'static FuncDef),
    Label(Name, u64),
}

pub struct Frame {
    binding: Binding,
    parent: Env,
}

pub type Env = Option<Rc<Frame>>;

/// The innermost frame whose binding satisfies `matches`.
fn find(env: &Env, matches: impl Fn(&Binding) -> bool) -> Option<&Rc<Frame>> {
    let mut frame = env.as_ref();
    while let Some(f) = frame {
        if matches(&f.binding) {
            return Some(f);
        }
        frame = f.parent.as_ref();
    }
    None
}

fn push(env: &Env, binding: Binding) -> Env {
    Some(Rc::new(Frame {
        binding,
        parent: env.clone(),
    }))
}

pub struct Program {
    main: Node,
    /// Whether the program reads further inputs itself (`input`, `inputs`).
    pub uses_inputs: bool,
    /// The variables defined before the program runs: `$ARGS` and the named arguments.
    root: Env,
    /// Supplies values for `input` and `inputs`.
    pub inputs: RefCell<Option<Records>>,
    /// The parse of each builtin written in the language, by registry position, once needed.
    definitions: Vec<OnceCell<&'static FuncDef>>,
    labels: Cell<u64>,
    recursion_depth: Cell<usize>,
    recursion_limit: usize,
    environment: OnceCell<Value>,
}

impl Program {
    pub fn compile(
        source: &str,
        environment: &Environment,
        recursion_limit: Option<usize>,
    ) -> AppResult<Self> {
        let source: &'static str = source.to_string().leak();
        let names: Vec<Name> = environment
            .named
            .iter()
            .map(|(name, _)| &*name.clone().leak())
            .collect();
        let variables = [&["ENV", "ARGS"][..], &names].concat();
        let parsed = parser::parse_program(source, &variables)?;

        let mut named = Map::new();
        let mut root = None;
        for (name, (_, text)) in names.iter().zip(&environment.named) {
            let value = parse_json(text, || format!("variable ${name}"))?;
            named.insert((*name).into(), value.clone());
            root = push(&root, Binding::Var(name, value));
        }
        let positional = environment
            .positional
            .iter()
            .map(|text| parse_json(text, || format!("positional argument {text}")));
        let mut args = Map::new();
        args.insert(
            "positional".into(),
            Value::arr(positional.collect::<AppResult<_>>()?),
        );
        args.insert("named".into(), Value::obj(named));
        root = push(&root, Binding::Var("ARGS", Value::obj(args)));

        Ok(Self {
            main: parsed.main,
            uses_inputs: parsed.uses_inputs,
            root,
            inputs: RefCell::new(None),
            definitions: builtins::registry()
                .iter()
                .map(|_| OnceCell::new())
                .collect(),
            labels: Cell::new(0),
            recursion_depth: Cell::new(0),
            recursion_limit: recursion_limit.unwrap_or(usize::MAX),
            environment: OnceCell::new(),
        })
    }

    /// The field names the program reads when it only does plain field access,
    /// so the parser can skip everything else.
    pub fn field_path(&self) -> Option<Vec<String>> {
        fn chain(ast: &Ast) -> Option<Vec<String>> {
            match ast {
                Ast::Identity => Some(Vec::new()),
                Ast::Index { target, key } => {
                    let Ast::Literal(Value::Str(name)) = &**key else {
                        return None;
                    };
                    let mut path = chain(target)?;
                    path.push(name.to_string());
                    Some(path)
                }
                Ast::Pipe(lhs, rhs) => {
                    let mut path = chain(lhs)?;
                    path.extend(chain(rhs)?);
                    Some(path)
                }
                _ => None,
            }
        }
        chain(self.main).filter(|path| !path.is_empty())
    }

    /// Runs the program on one input value, passing each output to `out`.
    pub fn run(&self, input: Value, out: &mut dyn FnMut(Value) -> R) -> R {
        self.eval(self.main, &self.root, Item::plain(input), &mut |item| {
            out(item.value)
        })
    }

    fn definition(&self, index: usize, source: &'static str) -> Result<&'static FuncDef, Stop> {
        if let Some(def) = self.definitions[index].get() {
            return Ok(def);
        }
        let def = parse_builtin_definition(source).map_err(|err| error(err.to_string()))?;
        Ok(*self.definitions[index].get_or_init(|| def))
    }

    pub fn environment(&self) -> Value {
        self.environment
            .get_or_init(|| {
                Value::obj(
                    std::env::vars()
                        .map(|(k, v)| (k.into(), Value::str(v)))
                        .collect(),
                )
            })
            .clone()
    }

    pub fn next_input(&self) -> Option<Result<Value, String>> {
        self.inputs
            .borrow_mut()
            .as_mut()
            .and_then(Records::next_input)
    }

    pub fn input_location(&self) -> String {
        self.inputs
            .borrow()
            .as_ref()
            .map_or_else(|| "<unknown>".into(), |source| source.location())
    }

    pub fn fresh_label(&self) -> u64 {
        self.labels.set(self.labels.get() + 1);
        self.labels.get()
    }

    fn enter_eval<T>(&self, f: impl FnOnce() -> Result<T, Stop>) -> Result<T, Stop> {
        stacker::maybe_grow(STACK_RED_ZONE, STACK_GROWTH, || {
            let depth = self.recursion_depth.get();
            if depth >= self.recursion_limit {
                return Err(error("recursion limit exceeded"));
            }
            self.recursion_depth.set(depth + 1);
            let result = f();
            self.recursion_depth.set(depth);
            result
        })
    }

    pub fn collect(&self, ast: Node, env: &Env, input: Item) -> Result<Vec<Value>, Stop> {
        let mut values = Vec::new();
        self.eval(ast, env, input, &mut |item| {
            values.push(item.value);
            Ok(())
        })?;
        Ok(values)
    }

    /// Runs `body`, passing its outputs to `out`. Returns the error `body` raised
    /// itself, if any; errors raised by `out` are not `body`'s and propagate.
    pub fn caught(
        &self,
        body: impl FnOnce(Out<'_>) -> R,
        out: Out<'_>,
    ) -> Result<Result<(), Value>, Stop> {
        let mut downstream = None;
        let result = body(&mut |item| {
            out(item).map_err(|stop| {
                downstream = Some(stop);
                Stop::Passthrough
            })
        });
        match result {
            Ok(()) => Ok(Ok(())),
            Err(Stop::Passthrough) => Err(downstream
                .take()
                .expect("a passthrough follows a downstream stop")),
            Err(Stop::Error(value)) => Ok(Err(value)),
            Err(other) => Err(other),
        }
    }

    /// Calls `f` with one output of each of `nodes`, for every combination of
    /// outputs. The last node varies slowest, as with the operators.
    pub fn combinations(
        &self,
        nodes: &[Node],
        env: &Env,
        input: &Value,
        f: &mut dyn FnMut(&[Value]) -> R,
    ) -> R {
        self.fill(nodes, env, input, &mut vec![Value::Null; nodes.len()], f)
    }

    /// Sets `values[i]` to each output of `nodes[i]` in turn, then calls `f`.
    fn fill(
        &self,
        nodes: &[Node],
        env: &Env,
        input: &Value,
        values: &mut [Value],
        f: &mut dyn FnMut(&[Value]) -> R,
    ) -> R {
        let Some((last, rest)) = nodes.split_last() else {
            return f(values);
        };
        self.eval(last, env, Item::plain(input.clone()), &mut |item| {
            values[rest.len()] = item.value;
            self.fill(rest, env, input, values, f)
        })
    }

    pub fn eval(&self, ast: Node, env: &Env, input: Item, out: Out<'_>) -> R {
        self.enter_eval(|| match ast {
            Ast::Identity => out(input),
            Ast::Recurse => self.descend(input, out),
            Ast::Literal(value) => out(input.derive(value.clone())),
            Ast::Format(name) => {
                let text = lift(library::format(name, &input.value))?;
                out(input.derive(Value::str(text)))
            }
            Ast::Interpolated(string) => {
                let plain = input.detached();
                let n = string.parts.len();
                self.interpolate(string, n, String::new(), env, &plain, &mut |text| {
                    out(input.derive(Value::str(text)))
                })
            }
            Ast::Index { target, key } => {
                let plain = input.detached();
                self.eval(key, env, plain, &mut |k| {
                    self.eval(target, env, input.clone(), &mut |t| {
                        self.index(t, &k.value, out)
                    })
                })
            }
            // Listed end first, so that the start varies slowest.
            Ast::Slice { target, from, to } => {
                self.combinations(&[to, from], env, &input.value, &mut |ends| {
                    let key = value::slice_key(ends[1].clone(), ends[0].clone());
                    self.eval(target, env, input.clone(), &mut |t| {
                        self.index(t, &key, out)
                    })
                })
            }
            Ast::Iterate(target) => self.eval(target, env, input, &mut |t| self.iterate(t, out)),
            Ast::Array(body) => {
                let items = self.collect(body, env, input.detached())?;
                out(input.derive(Value::arr(items)))
            }
            Ast::Object(pairs) => {
                let plain = input.detached();
                self.build_object(pairs, Map::new(), env, &plain, &mut |entries| {
                    out(input.derive(Value::obj(entries)))
                })
            }
            Ast::Negate(body) => self.eval(body, env, input.detached(), &mut |v| {
                out(input.derive(lift(value::negate(v.value))?))
            }),
            Ast::Pipe(lhs, rhs) => {
                self.eval(lhs, env, input, &mut |item| self.eval(rhs, env, item, out))
            }
            Ast::Comma(lhs, rhs) => {
                self.eval(lhs, env, input.clone(), out)?;
                self.eval(rhs, env, input, out)
            }
            Ast::Binary(op, lhs, rhs) => self.binary(*op, lhs, rhs, env, input, out),
            Ast::And(lhs, rhs) => self.logic(false, lhs, rhs, env, input, out),
            Ast::Or(lhs, rhs) => self.logic(true, lhs, rhs, env, input, out),
            Ast::Alternative(lhs, rhs) => self.alternative(lhs, rhs, env, input, out),
            Ast::Assign(op, lhs, rhs) => self.assign(*op, lhs, rhs, env, input, out),
            Ast::If {
                condition,
                then,
                otherwise,
            } => self.eval(condition, env, input.detached(), &mut |c| {
                if c.value.truthy() {
                    self.eval(then, env, input.clone(), out)
                } else if let Some(otherwise) = otherwise {
                    self.eval(otherwise, env, input.clone(), out)
                } else {
                    out(input.clone())
                }
            }),
            Ast::Try { body, handler } => {
                let outcome =
                    self.caught(|inner| self.eval(body, env, input.clone(), inner), out)?;
                match (outcome, handler) {
                    (Err(error), Some(handler)) => {
                        self.eval(handler, env, input.derive(error), out)
                    }
                    _ => Ok(()),
                }
            }
            Ast::Fold(fold) => self.fold(fold, env, input, out),
            Ast::Def { def, rest } => {
                let env = push(env, Binding::Func(def));
                self.eval(rest, &env, input, out)
            }
            Ast::Call { name, args } => self.call(name, args, env, input, out),
            Ast::Builtin { index, args } => self.builtin(*index, args, env, input, out),
            Ast::Var(name) => out(input.derive(self.variable(env, name)?)),
            Ast::Bind {
                source,
                patterns,
                body,
            } => {
                let plain = input.detached();
                self.eval(source, env, plain.clone(), &mut |item| {
                    let run_body = &mut |bound: Env, emit: Out<'_>| {
                        self.eval(body, &bound, input.clone(), emit)
                    };
                    self.bind(patterns, &item.value, env, &plain, run_body, out)
                })
            }
            Ast::Label { name, body } => {
                let id = self.fresh_label();
                let env = push(env, Binding::Label(name, id));
                match self.eval(body, &env, input, out) {
                    Err(Stop::Break(found)) if found == id => Ok(()),
                    other => other,
                }
            }
            Ast::Break(name) => Err(self.break_to(env, name)),
        })
    }

    fn builtin(&self, index: usize, args: &[Node], env: &Env, input: Item, out: Out<'_>) -> R {
        match &builtins::registry()[index].how {
            How::Plain(function) => self.combinations(args, env, &input.value, &mut |values| {
                let all = [std::slice::from_ref(&input.value), values].concat();
                out(input.derive(lift(function(&all))?))
            }),
            How::Special(special) => special(self, args, env, input, out),
            How::Source(source) => {
                let def = self.definition(index, source)?;
                let call_env = self.enter(def, args, env, &None);
                self.eval(def.body, &call_env, input, out)
            }
        }
    }

    /// The stop that `break $name` raises: it unwinds to the matching `label`.
    fn break_to(&self, env: &Env, name: &str) -> Stop {
        let found = find(
            env,
            |b| matches!(b, Binding::Label(label, _) if *label == name),
        );
        match found.map(|frame| &frame.binding) {
            Some(Binding::Label(_, id)) => Stop::Break(*id),
            _ => error(format!("$*label-{name} is not defined")),
        }
    }

    /// `lhs op rhs`: the right side varies slowest.
    fn binary(&self, op: Op, lhs: Node, rhs: Node, env: &Env, input: Item, out: Out<'_>) -> R {
        let plain = input.detached();
        let rhs_values = self.collect(rhs, env, plain.clone())?;
        for r in rhs_values {
            self.eval(lhs, env, plain.clone(), &mut |l| {
                out(input.derive(lift(apply(op, l.value, r.clone()))?))
            })?;
        }
        Ok(())
    }

    /// `and` (stops on a falsy left side) and `or` (stops on a truthy one).
    fn logic(
        &self,
        stops_on: bool,
        lhs: Node,
        rhs: Node,
        env: &Env,
        input: Item,
        out: Out<'_>,
    ) -> R {
        let plain = input.detached();
        self.eval(lhs, env, plain.clone(), &mut |l| {
            if l.value.truthy() == stops_on {
                return out(input.derive(Value::Bool(stops_on)));
            }
            self.eval(rhs, env, plain.clone(), &mut |r| {
                out(input.derive(Value::Bool(r.value.truthy())))
            })
        })
    }

    /// `lhs // rhs`: the truthy outputs of `lhs`, or `rhs` if there are none.
    /// Errors raised by `lhs` itself are suppressed.
    fn alternative(&self, lhs: Node, rhs: Node, env: &Env, input: Item, out: Out<'_>) -> R {
        let mut produced = false;
        let body = |inner: Out<'_>| {
            self.eval(lhs, env, input.clone(), &mut |item| {
                if !item.value.truthy() {
                    return Ok(());
                }
                produced = true;
                inner(item)
            })
        };
        self.caught(body, out)?.ok();
        if produced {
            Ok(())
        } else {
            self.eval(rhs, env, input, out)
        }
    }

    /// `reduce` and `foreach`: runs `update` over the state for each element of
    /// `source`. Reduce keeps the last result and emits the final state; foreach
    /// emits the (extracted) state after every update.
    fn fold(&self, fold: &Fold, env: &Env, input: Item, out: Out<'_>) -> R {
        let plain = input.detached();
        self.eval(fold.init, env, plain.clone(), &mut |start| {
            let mut state = start.value;
            self.eval(fold.source, env, plain.clone(), &mut |item| {
                let step = &mut |bound: Env, emit: Out<'_>| {
                    // Reduce takes the state so that appending can reuse it in place.
                    let current = if fold.each {
                        state.clone()
                    } else {
                        std::mem::take(&mut state)
                    };
                    self.eval(fold.update, &bound, Item::plain(current), &mut |next| {
                        if !fold.each {
                            state = next.value;
                            return Ok(());
                        }
                        state = next.value.clone();
                        match fold.extract {
                            Some(extract) => {
                                self.eval(extract, &bound, Item::plain(next.value), &mut |e| {
                                    emit(input.derive(e.value))
                                })
                            }
                            None => emit(input.derive(next.value)),
                        }
                    })
                };
                self.bind(fold.patterns, &item.value, env, &plain, step, out)
            })?;
            if fold.each {
                Ok(())
            } else {
                out(input.derive(state))
            }
        })
    }

    fn index(&self, target: Item, key: &Value, out: Out<'_>) -> R {
        if matches!(target.track, Track::Invalid) {
            return Err(error(format!(
                "Invalid path expression near attempt to access element {} of {}",
                shown(key),
                shown(&target.value)
            )));
        }
        let value = lift(value::index(&target.value, key))?;
        out(target.child(value, key.clone()))
    }

    fn iterate(&self, target: Item, out: Out<'_>) -> R {
        if matches!(target.track, Track::Invalid) {
            return Err(error(format!(
                "Invalid path expression near attempt to iterate through {}",
                shown(&target.value)
            )));
        }
        match &target.value {
            Value::Arr(items) => {
                for (i, child) in items.iter().enumerate() {
                    out(target.child(child.clone(), Value::num(i as f64)))?;
                }
                Ok(())
            }
            Value::Obj(map) => {
                for (key, child) in map.iter() {
                    out(target.child(child.clone(), Value::Str(key.clone())))?;
                }
                Ok(())
            }
            other => Err(error(format!("Cannot iterate over {}", describe(other)))),
        }
    }

    pub fn descend(&self, item: Item, out: Out<'_>) -> R {
        out(item.clone())?;
        match &item.value {
            Value::Arr(items) => {
                for (i, child) in items.iter().enumerate() {
                    self.descend(item.child(child.clone(), Value::num(i as f64)), out)?;
                }
            }
            Value::Obj(map) => {
                for (key, child) in map.iter() {
                    self.descend(item.child(child.clone(), Value::Str(key.clone())), out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Every path the expression selects in `value`.
    pub fn paths(&self, ast: Node, env: &Env, value: Value) -> Result<Vec<Vec<Value>>, Stop> {
        let mut paths = Vec::new();
        self.eval(
            ast,
            env,
            Item {
                value,
                track: Track::At(Vec::new()),
            },
            &mut |item| match item.track {
                Track::At(keys) => {
                    paths.push(keys);
                    Ok(())
                }
                _ => Err(invalid_path(&item.value)),
            },
        )?;
        Ok(paths)
    }

    /// Builds the string from its last part back, so that the last
    /// interpolation varies slowest.
    fn interpolate(
        &self,
        string: &Interpolation,
        remaining: usize,
        tail: String,
        env: &Env,
        input: &Item,
        done: &mut dyn FnMut(String) -> R,
    ) -> R {
        let Some(index) = remaining.checked_sub(1) else {
            return done(tail);
        };
        match &string.parts[index] {
            Part::Text(text) => {
                self.interpolate(string, index, format!("{text}{tail}"), env, input, done)
            }
            Part::Interpolation(node) => self.eval(node, env, input.clone(), &mut |item| {
                let text = match string.format {
                    Some(name) => lift(library::format(name, &item.value))?,
                    None => library::to_string(&item.value),
                };
                self.interpolate(string, index, format!("{text}{tail}"), env, input, done)
            }),
        }
    }

    fn build_object(
        &self,
        pairs: &[(Node, Node)],
        built: Map,
        env: &Env,
        input: &Item,
        done: &mut dyn FnMut(Map) -> R,
    ) -> R {
        let Some(((key, value), rest)) = pairs.split_first() else {
            return done(built);
        };
        self.eval(key, env, input.clone(), &mut |k| {
            let Value::Str(name) = k.value else {
                return Err(error("Object keys must be strings"));
            };
            self.eval(value, env, input.clone(), &mut |v| {
                let mut next = built.clone();
                next.insert(name.clone(), v.value);
                self.build_object(rest, next, env, input, done)
            })
        })
    }

    fn variable(&self, env: &Env, name: &str) -> Result<Value, Stop> {
        let found = find(env, |b| matches!(b, Binding::Var(var, _) if *var == name));
        match found.map(|frame| &frame.binding) {
            Some(Binding::Var(_, value)) => Ok(value.clone()),
            _ if name == "ENV" => Ok(self.environment()),
            _ => Err(error(format!("${name} is not defined"))),
        }
    }

    fn call(&self, name: &str, args: &[Node], env: &Env, input: Item, out: Out<'_>) -> R {
        let matches = |binding: &Binding| match binding {
            Binding::Closure(param, ..) => args.is_empty() && *param == name,
            Binding::Func(def) => def.name == name && def.params.len() == args.len(),
            _ => false,
        };
        let Some(frame) = find(env, matches) else {
            return Err(error(format!("{name}/{} is not defined", args.len())));
        };
        match &frame.binding {
            Binding::Closure(_, body, captured) => self.eval(body, captured, input, out),
            Binding::Func(def) => {
                let call_env = self.enter(def, args, env, &Some(frame.clone()));
                self.eval(def.body, &call_env, input, out)
            }
            _ => unreachable!("only closures and functions match"),
        }
    }

    /// The environment a function body runs in: its definition scope plus its
    /// parameters, bound to closures over the caller's arguments.
    fn enter(&self, def: &FuncDef, args: &[Node], caller: &Env, defined: &Env) -> Env {
        let mut env = defined.clone();
        for (param, arg) in def.params.iter().zip(args) {
            env = push(&env, Binding::Closure(param, arg, caller.clone()));
        }
        env
    }

    /// Binds the first of `patterns` (`?//` alternatives) that matches `value`
    /// and runs `body` for each binding. An error raised by `body` itself, not
    /// by `out`, moves on to the next alternative.
    fn bind(
        &self,
        patterns: &[Pattern],
        value: &Value,
        env: &Env,
        key_input: &Item,
        body: &mut dyn FnMut(Env, Out<'_>) -> R,
        out: Out<'_>,
    ) -> R {
        // With alternatives, every variable of every pattern is bound, to null
        // unless the pattern that matched sets it.
        let mut base = env.clone();
        if patterns.len() > 1 {
            let mut names = Vec::new();
            patterns.iter().for_each(|p| p.variables(&mut names));
            for name in names {
                base = push(&base, Binding::Var(name, Value::Null));
            }
        }
        let (last, earlier) = patterns.split_last().expect("a binding has a pattern");
        for pattern in earlier {
            let attempt = |inner: Out<'_>| {
                self.destructure(pattern, value, base.clone(), key_input, &mut |bound| {
                    body(bound, inner)
                })
            };
            if self.caught(attempt, out)?.is_ok() {
                return Ok(());
            }
        }
        self.destructure(last, value, base.clone(), key_input, &mut |bound| {
            body(bound, out)
        })
    }

    fn destructure(
        &self,
        pattern: &Pattern,
        value: &Value,
        env: Env,
        key_input: &Item,
        body: &mut dyn FnMut(Env) -> R,
    ) -> R {
        match pattern {
            Pattern::Var(name) => body(push(&env, Binding::Var(name, value.clone()))),
            Pattern::Array(items) => self.destructure_items(items, 0, value, env, key_input, body),
            Pattern::Object(entries) => {
                self.destructure_entries(entries, value, env, key_input, body)
            }
        }
    }

    fn destructure_items(
        &self,
        items: &[Pattern],
        position: usize,
        value: &Value,
        env: Env,
        key_input: &Item,
        body: &mut dyn FnMut(Env) -> R,
    ) -> R {
        let Some(pattern) = items.get(position) else {
            return body(env);
        };
        let element = lift(value::index(value, &Value::num(position as f64)))?;
        self.destructure(pattern, &element, env, key_input, &mut |bound| {
            self.destructure_items(items, position + 1, value, bound, key_input, body)
        })
    }

    fn destructure_entries(
        &self,
        entries: &[ObjectEntry],
        value: &Value,
        env: Env,
        key_input: &Item,
        body: &mut dyn FnMut(Env) -> R,
    ) -> R {
        let Some((entry, rest)) = entries.split_first() else {
            return body(env);
        };
        self.eval(entry.key, &env, key_input.clone(), &mut |key| {
            if !matches!(key.value, Value::Str(_)) {
                return Err(error(format!(
                    "Cannot index {} with {}",
                    value.kind(),
                    key.value.kind()
                )));
            }
            let element = lift(value::index(value, &key.value))?;
            let mut bound = env.clone();
            if let Some(var) = &entry.var {
                bound = push(&bound, Binding::Var(var, element.clone()));
            }
            let next = |bound: Env, body: &mut dyn FnMut(Env) -> R| {
                self.destructure_entries(rest, value, bound, key_input, body)
            };
            match &entry.pattern {
                Some(pattern) => {
                    self.destructure(pattern, &element, bound, key_input, &mut |bound| {
                        next(bound, body)
                    })
                }
                None => next(bound, body),
            }
        })
    }

    fn assign(&self, op: Assign, lhs: Node, rhs: Node, env: &Env, input: Item, out: Out<'_>) -> R {
        let plain = input.detached();
        match op {
            Assign::Update => {
                let result = self.modify(lhs, env, input.value.clone(), &mut |old| {
                    let mut first = None;
                    let id = self.fresh_label();
                    let outcome = self.eval(rhs, env, Item::plain(old), &mut |item| {
                        first = Some(item.value);
                        Err(Stop::Break(id))
                    });
                    match outcome {
                        Err(Stop::Break(found)) if found == id => Ok(first),
                        Err(other) => Err(other),
                        Ok(()) => Ok(first),
                    }
                })?;
                out(input.derive(result))
            }
            Assign::Set => self.eval(rhs, env, plain, &mut |new| {
                let mut state = input.value.clone();
                for path in self.paths(lhs, env, input.value.clone())? {
                    state = lift(value::set_path(state, &path, new.value.clone()))?;
                }
                out(input.derive(state))
            }),
            Assign::Arith(_) | Assign::Alt => self.eval(rhs, env, plain, &mut |operand| {
                let result = self.modify(lhs, env, input.value.clone(), &mut |old| {
                    let operand = operand.value.clone();
                    Ok(Some(match op {
                        Assign::Arith(op) => lift(apply(op, old, operand))?,
                        _ if old.truthy() => old,
                        _ => operand,
                    }))
                })?;
                out(input.derive(result))
            }),
        }
    }

    /// Replaces each value `lhs` selects with `update(value)`, deleting those
    /// for which it returns `None`.
    fn modify(
        &self,
        lhs: Node,
        env: &Env,
        root: Value,
        update: &mut dyn FnMut(Value) -> Result<Option<Value>, Stop>,
    ) -> Result<Value, Stop> {
        let mut state = root.clone();
        let mut deletions = Vec::new();
        for path in self.paths(lhs, env, root)? {
            let current = lift(value::get_path(&state, &path))?;
            match update(current)? {
                Some(new) => state = lift(value::set_path(state, &path, new))?,
                None => deletions.push(path),
            }
        }
        if deletions.is_empty() {
            Ok(state)
        } else {
            lift(value::delete_paths(state, deletions))
        }
    }
}

/// `value` as JSON, cut short to fit in an error message.
fn shown(value: &Value) -> String {
    truncate(dump(value), 30)
}

pub fn invalid_path(value: &Value) -> Stop {
    error(format!(
        "Invalid path expression with result {}",
        shown(value)
    ))
}

pub fn apply(op: Op, a: Value, b: Value) -> value::Res<Value> {
    let order = || compare(&a, &b);
    Ok(match op {
        Op::Add => return value::add(a, b),
        Op::Sub => return value::subtract(a, b),
        Op::Mul => return value::multiply(a, b),
        Op::Div => return value::divide(a, b),
        Op::Rem => return value::remainder(a, b),
        Op::Eq => Value::Bool(order().is_eq()),
        Op::Ne => Value::Bool(order().is_ne()),
        Op::Lt => Value::Bool(order().is_lt()),
        Op::Le => Value::Bool(order().is_le()),
        Op::Gt => Value::Bool(order().is_gt()),
        Op::Ge => Value::Bool(order().is_ge()),
    })
}

/// What a program may refer to that comes from the command line. Values are
/// kept as JSON text so the settings can be shared between threads.
#[derive(Clone, Default)]
pub struct Environment {
    /// `--arg`, `--argjson`, `--slurpfile` and `--rawfile` variables.
    pub named: Vec<(String, String)>,
    /// Positional arguments from `--args` and `--jsonargs`.
    pub positional: Vec<String>,
}

fn parse_json(text: &str, what: impl FnOnce() -> String) -> AppResult<Value> {
    reader::parse(text).map_err(|reason| Error::InvalidJson {
        what: what(),
        reason,
    })
}

pub fn error_line(value: &Value, location: &str) -> String {
    match value {
        Value::Str(message) => format!("jx: error (at {location}): {message}"),
        other => format!("jx: error (at {location}) (not a string): {}", dump(other)),
    }
}
