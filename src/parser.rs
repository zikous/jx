//! The grammar. Names are resolved while parsing, so unknown functions and
//! variables are reported as compile errors.

use crate::builtins;
use crate::error::{Error, Result};
use crate::reader;
use crate::value::{Map, Value};
use crate::writer;
use logos::Logos;

/// A token. Keywords are plain identifiers, which the parser tells apart.
/// Strings are not tokens: the parser reads their bodies itself, since they
/// can embed whole queries.
#[derive(Logos, Clone, Copy, Debug, PartialEq)]
#[logos(skip r"[ \t\r\n]+")]
#[logos(skip(r"#[^\n]*", allow_greedy = true))]
pub enum Tok<'s> {
    #[regex(r"([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?", |lex| lex.slice())]
    Num(&'s str),
    #[regex(r"\.[A-Za-z_][A-Za-z_0-9]*", |lex| &lex.slice()[1..])]
    Field(&'s str),
    #[regex(r"\$([A-Za-z_][A-Za-z_0-9]*::)*[A-Za-z_][A-Za-z_0-9]*", |lex| &lex.slice()[1..])]
    Var(&'s str),
    #[regex(r"([A-Za-z_][A-Za-z_0-9]*::)*[A-Za-z_][A-Za-z_0-9]*", |lex| lex.slice())]
    Ident(&'s str),
    #[regex(r"@[A-Za-z0-9_]+", |lex| &lex.slice()[1..])]
    Format(&'s str),
    #[regex(r#"\.\.|\?//|//=|\|=|[-+*/%=!<>]=|//|[.|,:;?()\[\]{}+\-*/%=<>"]"#, |lex| lex.slice())]
    Sym(&'s str),
    /// The end of the program; the lexer itself never produces it.
    Eof,
}

/// A program lives as long as the process, so its syntax tree is leaked once
/// and shared by plain reference.
pub type Name = &'static str;
pub type Node = &'static Ast;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Assign {
    Set,
    Update,
    Arith(Op),
    Alt,
}

/// `reduce` (emits the final state) or `foreach` (emits after each update).
pub struct Fold {
    pub each: bool,
    pub source: Node,
    pub patterns: &'static [Pattern],
    pub init: Node,
    pub update: Node,
    pub extract: Option<Node>,
}

/// A string with `\(...)` interpolations, optionally under a `@format`.
pub struct Interpolation {
    pub format: Option<Name>,
    pub parts: Vec<Part>,
}

pub enum Part {
    Text(String),
    Interpolation(Node),
}

pub enum Ast {
    Identity,
    Recurse,
    Literal(Value),
    /// `@name`, applied to the input.
    Format(Name),
    Interpolated(Interpolation),
    Index {
        target: Node,
        key: Node,
    },
    /// A missing bound is `null`.
    Slice {
        target: Node,
        from: Node,
        to: Node,
    },
    Iterate(Node),
    Array(Node),
    Object(Vec<(Node, Node)>),
    Negate(Node),
    Pipe(Node, Node),
    Comma(Node, Node),
    Binary(Op, Node, Node),
    And(Node, Node),
    Or(Node, Node),
    Alternative(Node, Node),
    Assign(Assign, Node, Node),
    If {
        condition: Node,
        then: Node,
        otherwise: Option<Node>,
    },
    Try {
        body: Node,
        handler: Option<Node>,
    },
    Fold(Fold),
    Def {
        def: &'static FuncDef,
        rest: Node,
    },
    /// A local function or a closure parameter, found by name at run time.
    Call {
        name: Name,
        args: Vec<Node>,
    },
    /// A builtin, by its position in the registry.
    Builtin {
        index: usize,
        args: Vec<Node>,
    },
    Var(Name),
    Bind {
        source: Node,
        patterns: &'static [Pattern],
        body: Node,
    },
    Label {
        name: Name,
        body: Node,
    },
    Break(Name),
}

pub struct FuncDef {
    pub name: Name,
    pub params: Vec<Name>,
    pub body: Node,
}

pub enum Pattern {
    Var(Name),
    Array(Vec<Pattern>),
    Object(Vec<ObjectEntry>),
}

pub struct ObjectEntry {
    /// The variable bound to the whole entry value, for `$name` forms.
    pub var: Option<Name>,
    pub key: Node,
    pub pattern: Option<Pattern>,
}

impl Pattern {
    /// Every variable the pattern can bind.
    pub fn variables(&self, into: &mut Vec<Name>) {
        match self {
            Self::Var(name) => into.push(name),
            Self::Array(items) => items.iter().for_each(|p| p.variables(into)),
            Self::Object(entries) => {
                for entry in entries {
                    into.extend(entry.var);
                    if let Some(pattern) = &entry.pattern {
                        pattern.variables(into);
                    }
                }
            }
        }
    }
}

pub struct Parsed {
    pub main: Node,
    /// Whether the program reads further inputs itself (`input`, `inputs`).
    pub uses_inputs: bool,
}

/// Parses a program in which `variables` (`$ENV`, `--arg` names, ...) are always defined.
pub fn parse_program(source: &'static str, variables: &[Name]) -> Result<Parsed> {
    let mut parser = Parser::new(source);
    parser
        .scope
        .extend(variables.iter().map(|&v| Scoped::Var(v)));
    let main = if matches!(parser.peek()?, Tok::Eof) {
        node(Ast::Identity)
    } else {
        let body = parser.pipe()?;
        parser.expect(Tok::Eof)?;
        body
    };
    Ok(Parsed {
        main,
        uses_inputs: parser.uses_inputs,
    })
}

/// Parses the definition of a builtin written in the language, whose own name
/// resolves through the registry.
pub fn parse_builtin_definition(source: &'static str) -> Result<&'static FuncDef> {
    let mut parser = Parser::new(source);
    let def = parser.definition(false)?;
    parser.expect(Tok::Eof)?;
    Ok(def)
}

enum Scoped {
    Var(Name),
    Func(Name, usize),
    Label(Name),
}

struct Parser {
    lexer: logos::Lexer<'static, Tok<'static>>,
    peeked: Option<(Tok<'static>, usize)>,
    scope: Vec<Scoped>,
    uses_inputs: bool,
}

fn leak<T>(value: T) -> &'static T {
    Box::leak(Box::new(value))
}

fn node(ast: Ast) -> Node {
    leak(ast)
}

fn literal(value: Value) -> Node {
    node(Ast::Literal(value))
}

fn string_literal(text: &str) -> Node {
    literal(Value::str(text))
}

impl Parser {
    fn new(source: &'static str) -> Self {
        Self {
            lexer: Tok::lexer(source),
            peeked: None,
            scope: Vec::new(),
            uses_inputs: false,
        }
    }

    fn peek(&mut self) -> Result<Tok<'static>> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lex()?);
        }
        Ok(self.peeked.expect("just filled").0)
    }

    fn next(&mut self) -> Result<(Tok<'static>, usize)> {
        match self.peeked.take() {
            Some(token) => Ok(token),
            None => self.lex(),
        }
    }

    fn eat(&mut self, tok: Tok) -> Result<bool> {
        let found = self.peek()? == tok;
        if found {
            self.next()?;
        }
        Ok(found)
    }

    fn expect(&mut self, tok: Tok) -> Result<()> {
        if self.eat(tok)? {
            return Ok(());
        }
        let (found, at) = self.next()?;
        Err(self.unexpected(found, at))
    }

    fn sym(&mut self, sym: &str) -> Result<bool> {
        self.eat(Tok::Sym(sym))
    }

    fn expect_sym(&mut self, sym: &str) -> Result<()> {
        self.expect(Tok::Sym(sym))
    }

    fn eat_keyword(&mut self, word: &str) -> Result<bool> {
        self.eat(Tok::Ident(word))
    }

    fn expect_keyword(&mut self, word: &str) -> Result<()> {
        self.expect(Tok::Ident(word))
    }

    fn unexpected(&self, tok: Tok, at: usize) -> Error {
        let what = match tok {
            Tok::Eof => "end of file".to_string(),
            Tok::Sym(s) | Tok::Ident(s) | Tok::Num(s) => s.to_string(),
            other => format!("{other:?}"),
        };
        Error::Unexpected {
            found: what,
            line: self.line_at(at),
        }
    }

    /// The next token and where it starts.
    fn lex(&mut self) -> Result<(Tok<'static>, usize)> {
        match self.lexer.next() {
            Some(Ok(tok)) => Ok((tok, self.lexer.span().start)),
            Some(Err(())) => Err(Error::Unexpected {
                found: "INVALID_CHARACTER".into(),
                line: self.line_at(self.lexer.span().start),
            }),
            None => Ok((Tok::Eof, self.lexer.source().len())),
        }
    }

    fn line_at(&self, offset: usize) -> usize {
        let source = self.lexer.source();
        source[..offset.min(source.len())].matches('\n').count() + 1
    }

    fn syntax_error(&self, offset: usize, message: &'static str) -> Error {
        Error::Syntax {
            message,
            line: self.line_at(offset),
        }
    }

    fn name(&mut self) -> Result<Name> {
        match self.next()? {
            (Tok::Ident(name) | Tok::Var(name), _) => Ok(name),
            (other, at) => Err(self.unexpected(other, at)),
        }
    }

    /// `def name(params): body;`. The name is visible inside its own body when
    /// `recursive`; a builtin's own name resolves through the registry instead.
    fn definition(&mut self, recursive: bool) -> Result<&'static FuncDef> {
        self.expect_keyword("def")?;
        let name = self.name()?;
        // Each parameter, and whether it is a `$value` parameter.
        let mut params: Vec<(Name, bool)> = Vec::new();
        if self.sym("(")? {
            loop {
                match self.next()? {
                    (Tok::Ident(p), _) => params.push((p, false)),
                    (Tok::Var(p), _) => params.push((p, true)),
                    (other, at) => return Err(self.unexpected(other, at)),
                }
                if !self.sym(";")? {
                    break;
                }
            }
            self.expect_sym(")")?;
        }
        self.expect_sym(":")?;

        let mark = self.scope.len();
        if recursive {
            self.scope.push(Scoped::Func(name, params.len()));
        }
        for (param, is_value) in &params {
            self.scope.push(Scoped::Func(param, 0));
            if *is_value {
                self.scope.push(Scoped::Var(param));
            }
        }
        let mut body = self.pipe()?;
        self.expect_sym(";")?;
        self.scope.truncate(mark);

        // `def f($a): body` is `def f(a): a as $a | body`.
        for (param, _) in params.iter().rev().filter(|(_, is_value)| *is_value) {
            let source = node(Ast::Call {
                name: param,
                args: Vec::new(),
            });
            let patterns = leak([Pattern::Var(param)]);
            body = node(Ast::Bind {
                source,
                patterns,
                body,
            });
        }
        Ok(leak(FuncDef {
            name,
            params: params.into_iter().map(|(p, _)| p).collect(),
            body,
        }))
    }

    fn pipe(&mut self) -> Result<Node> {
        if self.peek()? == Tok::Ident("def") {
            let def = self.definition(true)?;
            let mark = self.scope.len();
            self.scope.push(Scoped::Func(def.name, def.params.len()));
            if matches!(self.peek()?, Tok::Eof) {
                return Err(Error::NoMainExpression);
            }
            let rest = self.pipe()?;
            self.scope.truncate(mark);
            return Ok(node(Ast::Def { def, rest }));
        }
        if self.eat_keyword("label")? {
            let name = self.name()?;
            self.expect_sym("|")?;
            self.scope.push(Scoped::Label(name));
            let body = self.pipe()?;
            self.scope.pop();
            return Ok(node(Ast::Label { name, body }));
        }
        let lhs = self.comma()?;
        if self.sym("|")? {
            let rhs = self.pipe()?;
            return Ok(node(Ast::Pipe(lhs, rhs)));
        }
        Ok(lhs)
    }

    fn comma(&mut self) -> Result<Node> {
        let mut lhs = self.alternative()?;
        while self.sym(",")? {
            // A definition or label after a comma scopes over everything that follows.
            let rhs = if matches!(self.peek()?, Tok::Ident("def" | "label")) {
                self.pipe()?
            } else {
                self.alternative()?
            };
            lhs = node(Ast::Comma(lhs, rhs));
        }
        Ok(lhs)
    }

    fn alternative(&mut self) -> Result<Node> {
        let lhs = self.assignment()?;
        if self.sym("//")? {
            return Ok(node(Ast::Alternative(lhs, self.alternative()?)));
        }
        Ok(lhs)
    }

    fn assignment(&mut self) -> Result<Node> {
        let lhs = self.or()?;
        let op = match self.peek()? {
            Tok::Sym("=") => Assign::Set,
            Tok::Sym("|=") => Assign::Update,
            Tok::Sym("+=") => Assign::Arith(Op::Add),
            Tok::Sym("-=") => Assign::Arith(Op::Sub),
            Tok::Sym("*=") => Assign::Arith(Op::Mul),
            Tok::Sym("/=") => Assign::Arith(Op::Div),
            Tok::Sym("%=") => Assign::Arith(Op::Rem),
            Tok::Sym("//=") => Assign::Alt,
            _ => return Ok(lhs),
        };
        self.next()?;
        Ok(node(Ast::Assign(op, lhs, self.or()?)))
    }

    fn or(&mut self) -> Result<Node> {
        let mut lhs = self.and()?;
        while self.eat_keyword("or")? {
            lhs = node(Ast::Or(lhs, self.and()?));
        }
        Ok(lhs)
    }

    fn and(&mut self) -> Result<Node> {
        let mut lhs = self.comparison()?;
        while self.eat_keyword("and")? {
            lhs = node(Ast::And(lhs, self.comparison()?));
        }
        Ok(lhs)
    }

    fn comparison(&mut self) -> Result<Node> {
        let lhs = self.additive()?;
        let op = match self.peek()? {
            Tok::Sym("==") => Op::Eq,
            Tok::Sym("!=") => Op::Ne,
            Tok::Sym("<") => Op::Lt,
            Tok::Sym("<=") => Op::Le,
            Tok::Sym(">") => Op::Gt,
            Tok::Sym(">=") => Op::Ge,
            _ => return Ok(lhs),
        };
        self.next()?;
        Ok(node(Ast::Binary(op, lhs, self.additive()?)))
    }

    fn additive(&mut self) -> Result<Node> {
        let mut lhs = self.multiplicative()?;
        loop {
            let op = match self.peek()? {
                Tok::Sym("+") => Op::Add,
                Tok::Sym("-") => Op::Sub,
                _ => return Ok(lhs),
            };
            self.next()?;
            lhs = node(Ast::Binary(op, lhs, self.multiplicative()?));
        }
    }

    fn multiplicative(&mut self) -> Result<Node> {
        let mut lhs = self.postfix(true)?;
        loop {
            let op = match self.peek()? {
                Tok::Sym("*") => Op::Mul,
                Tok::Sym("/") => Op::Div,
                Tok::Sym("%") => Op::Rem,
                _ => return Ok(lhs),
            };
            self.next()?;
            lhs = node(Ast::Binary(op, lhs, self.postfix(true)?));
        }
    }

    /// A term with its suffixes. With `binding`, a following `as` makes it a
    /// destructuring binding over the rest of the pipeline.
    fn postfix(&mut self, binding: bool) -> Result<Node> {
        let mut term = self.term()?;
        loop {
            match self.peek()? {
                Tok::Field(name) => {
                    self.next()?;
                    term = node(Ast::Index {
                        target: term,
                        key: string_literal(name),
                    });
                }
                Tok::Sym(".") => {
                    self.next()?;
                    if self.sym("\"")? {
                        let key = self.string_body(None)?;
                        term = node(Ast::Index { target: term, key });
                    } else if self.peek()? != Tok::Sym("[") {
                        let (tok, at) = self.next()?;
                        return Err(self.unexpected(tok, at));
                    }
                }
                Tok::Sym("[") => {
                    self.next()?;
                    term = self.bracket(term)?;
                }
                Tok::Sym("?") => {
                    self.next()?;
                    term = node(Ast::Try {
                        body: term,
                        handler: None,
                    });
                }
                _ => break,
            }
        }
        if binding && self.eat_keyword("as")? {
            let patterns = self.patterns()?;
            self.expect_sym("|")?;
            let mark = self.scope.len();
            self.bind_variables(patterns);
            let body = self.pipe()?;
            self.scope.truncate(mark);
            return Ok(node(Ast::Bind {
                source: term,
                patterns,
                body,
            }));
        }
        Ok(term)
    }

    /// After `[`: an iteration, an index or a slice.
    fn bracket(&mut self, target: Node) -> Result<Node> {
        if self.sym("]")? {
            return Ok(node(Ast::Iterate(target)));
        }
        let from = if self.sym(":")? {
            literal(Value::Null)
        } else {
            let from = self.pipe()?;
            if self.sym("]")? {
                return Ok(node(Ast::Index { target, key: from }));
            }
            self.expect_sym(":")?;
            from
        };
        let to = if self.peek()? == Tok::Sym("]") {
            literal(Value::Null)
        } else {
            self.pipe()?
        };
        self.expect_sym("]")?;
        Ok(node(Ast::Slice { target, from, to }))
    }

    fn term(&mut self) -> Result<Node> {
        let (tok, at) = self.next()?;
        Ok(match tok {
            Tok::Num(text) => literal(writer::number(text)),
            Tok::Sym("\"") => self.string_body(None)?,
            Tok::Format(name) => {
                if self.sym("\"")? {
                    self.string_body(Some(name))?
                } else {
                    node(Ast::Format(name))
                }
            }
            Tok::Sym(".") => {
                if self.sym("\"")? {
                    let key = self.string_body(None)?;
                    node(Ast::Index {
                        target: node(Ast::Identity),
                        key,
                    })
                } else {
                    node(Ast::Identity)
                }
            }
            Tok::Field(name) => node(Ast::Index {
                target: node(Ast::Identity),
                key: string_literal(name),
            }),
            Tok::Sym("..") => node(Ast::Recurse),
            Tok::Sym("-") => node(Ast::Negate(self.postfix(true)?)),
            Tok::Var(name) => self.variable(name, at)?,
            Tok::Sym("(") => {
                let body = self.pipe()?;
                self.expect_sym(")")?;
                body
            }
            Tok::Sym("[") => {
                if self.sym("]")? {
                    literal(Value::arr(Vec::new()))
                } else {
                    let body = self.pipe()?;
                    self.expect_sym("]")?;
                    node(Ast::Array(body))
                }
            }
            Tok::Sym("{") => self.object()?,
            Tok::Ident(name) => self.word(name, at)?,
            other => return Err(self.unexpected(other, at)),
        })
    }

    /// Keywords that start a term, literals, and calls.
    fn word(&mut self, name: Name, at: usize) -> Result<Node> {
        match name {
            "true" => return Ok(literal(Value::Bool(true))),
            "false" => return Ok(literal(Value::Bool(false))),
            "null" => return Ok(literal(Value::Null)),
            "if" => return self.conditional(),
            "try" => {
                let body = self.postfix(false)?;
                let handler = if self.eat_keyword("catch")? {
                    Some(self.postfix(false)?)
                } else {
                    None
                };
                return Ok(node(Ast::Try { body, handler }));
            }
            "reduce" => return self.fold(false),
            "foreach" => return self.fold(true),
            "break" => {
                let (Tok::Var(label), at) = self.next()? else {
                    return Err(self.syntax_error(at, "syntax error, unexpected break"));
                };
                if !self
                    .scope
                    .iter()
                    .any(|s| matches!(s, Scoped::Label(l) if *l == label))
                {
                    return Err(Error::UndefinedLabel {
                        name: label.to_string(),
                        line: self.line_at(at),
                    });
                }
                return Ok(node(Ast::Break(label)));
            }
            "as" | "then" | "elif" | "else" | "end" | "and" | "or" | "catch" | "def" | "label" => {
                return Err(self.unexpected(Tok::Ident(name), at));
            }
            _ => {}
        }
        let mut args = Vec::new();
        if self.sym("(")? {
            loop {
                args.push(self.pipe()?);
                if !self.sym(";")? {
                    break;
                }
            }
            self.expect_sym(")")?;
        }
        self.call(name, args, at)
    }

    fn call(&mut self, name: Name, args: Vec<Node>, at: usize) -> Result<Node> {
        let arity = args.len();
        let local = self
            .scope
            .iter()
            .rev()
            .any(|s| matches!(s, Scoped::Func(f, n) if *f == name && *n == arity));
        if local {
            return Ok(node(Ast::Call { name, args }));
        }
        if let Some(index) = builtins::resolve(name, arity) {
            self.uses_inputs |= matches!(
                name,
                "input" | "inputs" | "input_filename" | "input_line_number"
            );
            return Ok(node(Ast::Builtin { index, args }));
        }
        Err(Error::UndefinedFunction {
            name: name.to_string(),
            arity,
            line: self.line_at(at),
        })
    }

    fn variable(&mut self, name: Name, at: usize) -> Result<Node> {
        if name == "__loc__" {
            let mut location = Map::new();
            location.insert("file".into(), Value::str("<top-level>"));
            location.insert("line".into(), Value::num(self.line_at(at) as f64));
            return Ok(literal(Value::obj(location)));
        }
        if self
            .scope
            .iter()
            .any(|s| matches!(s, Scoped::Var(v) if *v == name))
        {
            Ok(node(Ast::Var(name)))
        } else {
            Err(Error::UndefinedVariable {
                name: name.to_string(),
                line: self.line_at(at),
            })
        }
    }

    fn conditional(&mut self) -> Result<Node> {
        let condition = self.pipe()?;
        self.expect_keyword("then")?;
        let then = self.pipe()?;
        let otherwise = if self.eat_keyword("elif")? {
            Some(self.conditional()?)
        } else if self.eat_keyword("else")? {
            let body = self.pipe()?;
            self.expect_keyword("end")?;
            Some(body)
        } else {
            self.expect_keyword("end")?;
            None
        };
        Ok(node(Ast::If {
            condition,
            then,
            otherwise,
        }))
    }

    /// `reduce SOURCE as PATTERN (INIT; UPDATE)` and `foreach` with an optional extract.
    fn fold(&mut self, each: bool) -> Result<Node> {
        let source = self.postfix(false)?;
        self.expect_keyword("as")?;
        let patterns = self.patterns()?;
        self.expect_sym("(")?;
        let init = self.pipe()?;
        self.expect_sym(";")?;
        let mark = self.scope.len();
        self.bind_variables(patterns);
        let update = self.pipe()?;
        let extract = if each && self.sym(";")? {
            Some(self.pipe()?)
        } else {
            None
        };
        self.scope.truncate(mark);
        self.expect_sym(")")?;
        Ok(node(Ast::Fold(Fold {
            each,
            source,
            patterns,
            init,
            update,
            extract,
        })))
    }

    fn object(&mut self) -> Result<Node> {
        let mut pairs = Vec::new();
        if !self.sym("}")? {
            loop {
                pairs.push(self.object_entry()?);
                if !self.sym(",")? {
                    self.expect_sym("}")?;
                    break;
                }
            }
        }
        Ok(node(Ast::Object(pairs)))
    }

    /// One `key: value`, or a shorthand: `{a}` is `{a: .a}` and `{$x}` is `{x: $x}`.
    fn object_entry(&mut self) -> Result<(Node, Node)> {
        let (tok, at) = self.next()?;
        let (key, can_shorten) = match tok {
            Tok::Var(name) => {
                let variable = self.variable(name, at)?;
                if self.sym(":")? {
                    return Ok((variable, self.object_value()?));
                }
                return Ok((string_literal(name), variable));
            }
            Tok::Ident(name) => (string_literal(name), true),
            Tok::Sym("\"") => (self.string_body(None)?, true),
            Tok::Format(name) => {
                self.expect_sym("\"")?;
                (self.string_body(Some(name))?, false)
            }
            Tok::Sym("(") => {
                let key = self.pipe()?;
                self.expect_sym(")")?;
                (key, false)
            }
            Tok::Num(_) => return Err(self.syntax_error(at, "Object keys must be strings")),
            other => return Err(self.unexpected(other, at)),
        };
        if self.sym(":")? {
            return Ok((key, self.object_value()?));
        }
        if !can_shorten {
            let (tok, at) = self.next()?;
            return Err(self.unexpected(tok, at));
        }
        let value = node(Ast::Index {
            target: node(Ast::Identity),
            key,
        });
        Ok((key, value))
    }

    /// A value in an object: anything but a comma, with `|` allowed.
    fn object_value(&mut self) -> Result<Node> {
        let mut value = self.alternative()?;
        while self.sym("|")? {
            value = node(Ast::Pipe(value, self.alternative()?));
        }
        Ok(value)
    }

    /// The rest of a string after its opening quote. The lexer is driven by
    /// hand here because interpolations contain whole queries.
    fn string_body(&mut self, format: Option<Name>) -> Result<Node> {
        let mut parts = Vec::new();
        let mut raw = String::new();
        loop {
            let rest = self.lexer.remainder();
            let mut chars = rest.char_indices();
            let Some((i, c)) = chars.next() else {
                return Err(self.syntax_error(self.lexer.span().end, "unterminated string literal"));
            };
            match c {
                '"' => {
                    self.lexer.bump(i + 1);
                    break;
                }
                '\\' if rest[1..].starts_with('(') => {
                    if !raw.is_empty() {
                        parts.push(Part::Text(self.unescape(&std::mem::take(&mut raw))?));
                    }
                    self.lexer.bump(2);
                    let inner = self.pipe()?;
                    self.expect_sym(")")?;
                    parts.push(Part::Interpolation(inner));
                }
                '\\' => {
                    let escaped = rest[1..].chars().next().map_or(0, char::len_utf8);
                    raw.push_str(&rest[..1 + escaped]);
                    self.lexer.bump(1 + escaped);
                }
                c => {
                    raw.push(c);
                    self.lexer.bump(c.len_utf8());
                }
            }
        }
        if !raw.is_empty() || parts.is_empty() {
            parts.push(Part::Text(self.unescape(&raw)?));
        }
        if let [Part::Text(text)] = parts.as_slice() {
            return Ok(string_literal(text));
        }
        Ok(node(Ast::Interpolated(Interpolation { format, parts })))
    }

    fn unescape(&self, raw: &str) -> Result<String> {
        match reader::parse(&format!("\"{raw}\"")) {
            Ok(Value::Str(s)) => Ok(s.to_string()),
            _ => Err(self.syntax_error(self.lexer.span().end, "Invalid escape in string literal")),
        }
    }

    fn patterns(&mut self) -> Result<&'static [Pattern]> {
        let mut patterns = vec![self.pattern()?];
        while self.sym("?//")? {
            patterns.push(self.pattern()?);
        }
        Ok(Vec::leak(patterns))
    }

    fn bind_variables(&mut self, patterns: &[Pattern]) {
        let mut names = Vec::new();
        patterns.iter().for_each(|p| p.variables(&mut names));
        self.scope.extend(names.into_iter().map(Scoped::Var));
    }

    fn pattern(&mut self) -> Result<Pattern> {
        let (tok, at) = self.next()?;
        match tok {
            Tok::Var(name) => Ok(Pattern::Var(name)),
            Tok::Sym("[") => {
                let mut items = vec![self.pattern()?];
                while self.sym(",")? {
                    items.push(self.pattern()?);
                }
                self.expect_sym("]")?;
                Ok(Pattern::Array(items))
            }
            Tok::Sym("{") => {
                let mut entries = vec![self.object_pattern_entry()?];
                while self.sym(",")? {
                    entries.push(self.object_pattern_entry()?);
                }
                self.expect_sym("}")?;
                Ok(Pattern::Object(entries))
            }
            other => Err(self.unexpected(other, at)),
        }
    }

    fn object_pattern_entry(&mut self) -> Result<ObjectEntry> {
        let (tok, at) = self.next()?;
        let (var, key) = match tok {
            Tok::Var(name) => (Some(name), string_literal(name)),
            Tok::Ident(name) => (None, string_literal(name)),
            Tok::Sym("\"") => (None, self.string_body(None)?),
            Tok::Sym("(") => {
                let key = self.pipe()?;
                self.expect_sym(")")?;
                (None, key)
            }
            other => return Err(self.unexpected(other, at)),
        };
        let pattern = if self.sym(":")? {
            Some(self.pattern()?)
        } else if var.is_some() {
            None
        } else {
            let (tok, at) = self.next()?;
            return Err(self.unexpected(tok, at));
        };
        Ok(ObjectEntry { var, key, pattern })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &'static str) -> Result<Parsed> {
        parse_program(source, &["ENV"])
    }

    #[test]
    fn precedence_binds_arithmetic_tighter_than_comparison_and_pipe() {
        let parsed = parse(". + 1 * 2 == 3 | not").unwrap();
        assert!(matches!(parsed.main, Ast::Pipe(lhs, _) if matches!(lhs, Ast::Binary(Op::Eq, ..))));
    }

    #[test]
    fn unknown_names_are_compile_errors() {
        assert!(
            parse("nosuch")
                .err()
                .expect("an error")
                .to_string()
                .contains("nosuch/0 is not defined")
        );
        assert!(
            parse("$nope")
                .err()
                .expect("an error")
                .to_string()
                .contains("$nope is not defined")
        );
    }

    #[test]
    fn a_definition_alone_is_not_a_program() {
        assert!(parse("def f: 1;").is_err());
    }

    #[test]
    fn string_interpolation_nests_queries() {
        let parsed = parse(r#""a\(1 + 2)b\("x")""#).unwrap();
        assert!(matches!(parsed.main, Ast::Interpolated(i) if i.parts.len() == 4));
    }
}
