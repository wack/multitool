//! Grammar:
//!
//! ```text
//! program   := item*
//! item      := decl | clause
//! decl      := ".decl" IDENT "(" [param ("," param)*] ")" ("@" IDENT)* ["."]
//!              (declaration annotations must be on the same line as the `)`)
//! param     := IDENT ":" ("int" | "string" | "symbol" | "term")
//! clause    := ("@" IDENT)* [IDENT ":"] head [":-" literal ("," literal)*] "."
//! head      := IDENT ["(" headarg ("," headarg)* ")"]
//! headarg   := ("count" | "sum" | "min" | "max") "<" VAR ">" | term
//! literal   := "not" atom | term CMP term | atom
//! term      := VAR | "_" | INT | STRING | IDENT ["(" term ("," term)* ")"]
//! ```
//!
//! Variables start with an uppercase letter or `_`; identifiers start with a
//! lowercase letter. Comments start with `%` or `//`. A clause with no body,
//! name, or annotations is a fact and must be ground.

use std::fmt;

use crate::ast::*;
use crate::value::Value;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub struct ParseError {
    pub line: usize,
    pub col: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Var(String),
    Int(i64),
    Str(String),
    LParen,
    RParen,
    Comma,
    Dot,
    Colon,
    ColonDash,
    At,
    Cmp(CmpOp),
    DeclKw,
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Ident(s) => write!(f, "identifier `{s}`"),
            Tok::Var(s) => write!(f, "variable `{s}`"),
            Tok::Int(i) => write!(f, "integer `{i}`"),
            Tok::Str(s) => write!(f, "string {s:?}"),
            Tok::LParen => f.write_str("`(`"),
            Tok::RParen => f.write_str("`)`"),
            Tok::Comma => f.write_str("`,`"),
            Tok::Dot => f.write_str("`.`"),
            Tok::Colon => f.write_str("`:`"),
            Tok::ColonDash => f.write_str("`:-`"),
            Tok::At => f.write_str("`@`"),
            Tok::Cmp(op) => write!(f, "`{op}`"),
            Tok::DeclKw => f.write_str("`.decl`"),
            Tok::Eof => f.write_str("end of input"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Pos {
    line: usize,
    col: usize,
}

fn err<T>(p: Pos, msg: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        line: p.line,
        col: p.col,
        msg: msg.into(),
    })
}

fn lex(src: &str) -> Result<Vec<(Tok, Pos)>, ParseError> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';

    macro_rules! bump {
        () => {{
            if cs[i] == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
        }};
    }

    while i < cs.len() {
        let c = cs[i];
        let pos = Pos { line, col };
        if c.is_whitespace() {
            bump!();
            continue;
        }
        if c == '%' || (c == '/' && cs.get(i + 1) == Some(&'/')) {
            while i < cs.len() && cs[i] != '\n' {
                bump!();
            }
            continue;
        }
        if c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == '_' {
            let start = i;
            while i < cs.len() && is_word(cs[i]) {
                bump!();
            }
            let w: String = cs[start..i].iter().collect();
            out.push((
                if c.is_ascii_lowercase() {
                    Tok::Ident(w)
                } else {
                    Tok::Var(w)
                },
                pos,
            ));
            continue;
        }
        if c.is_ascii_digit() || (c == '-' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            let start = i;
            bump!();
            while i < cs.len() && cs[i].is_ascii_digit() {
                bump!();
            }
            let w: String = cs[start..i].iter().collect();
            match w.parse::<i64>() {
                Ok(n) => out.push((Tok::Int(n), pos)),
                Err(_) => return err(pos, format!("integer out of range: {w}")),
            }
            continue;
        }
        if c == '"' {
            bump!();
            let mut s = String::new();
            loop {
                match cs.get(i) {
                    None => return err(pos, "unterminated string"),
                    Some('"') => {
                        bump!();
                        break;
                    }
                    Some('\\') => {
                        bump!();
                        let e = match cs.get(i) {
                            Some('"') => '"',
                            Some('\\') => '\\',
                            Some('n') => '\n',
                            Some('t') => '\t',
                            _ => return err(Pos { line, col }, "invalid escape in string"),
                        };
                        s.push(e);
                        bump!();
                    }
                    Some(&ch) => {
                        s.push(ch);
                        bump!();
                    }
                }
            }
            out.push((Tok::Str(s), pos));
            continue;
        }
        let two: String = cs[i..(i + 2).min(cs.len())].iter().collect();
        let (tok, len) = match (c, two.as_str()) {
            (_, ":-") => (Tok::ColonDash, 2),
            (_, "<=") => (Tok::Cmp(CmpOp::Le), 2),
            (_, ">=") => (Tok::Cmp(CmpOp::Ge), 2),
            (_, "!=") => (Tok::Cmp(CmpOp::Ne), 2),
            ('.', _) => {
                let rest: String = cs[i + 1..(i + 5).min(cs.len())].iter().collect();
                if rest == "decl" && !cs.get(i + 5).is_some_and(|&c| is_word(c)) {
                    (Tok::DeclKw, 5)
                } else {
                    (Tok::Dot, 1)
                }
            }
            ('(', _) => (Tok::LParen, 1),
            (')', _) => (Tok::RParen, 1),
            (',', _) => (Tok::Comma, 1),
            (':', _) => (Tok::Colon, 1),
            ('@', _) => (Tok::At, 1),
            ('=', _) => (Tok::Cmp(CmpOp::Eq), 1),
            ('<', _) => (Tok::Cmp(CmpOp::Lt), 1),
            ('>', _) => (Tok::Cmp(CmpOp::Gt), 1),
            _ => return err(pos, format!("unexpected character `{c}`")),
        };
        for _ in 0..len {
            bump!();
        }
        out.push((tok, pos));
    }
    out.push((Tok::Eof, Pos { line, col }));
    Ok(out)
}

/// A top-level item in source text.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Decl(Decl),
    /// A rule. The name is empty if the source didn't give one.
    Rule(Rule),
    Fact(Fact),
}

struct Parser {
    toks: Vec<(Tok, Pos)>,
    i: usize,
    wildcards: usize,
}

impl Parser {
    fn new(src: &str) -> Result<Self, ParseError> {
        Ok(Parser {
            toks: lex(src)?,
            i: 0,
            wildcards: 0,
        })
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.i].0
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.i + n).min(self.toks.len() - 1)].0
    }

    fn pos(&self) -> Pos {
        self.toks[self.i].1
    }

    fn next(&mut self) -> Tok {
        let t = self.toks[self.i].0.clone();
        if self.i < self.toks.len() - 1 {
            self.i += 1;
        }
        t
    }

    fn expect(&mut self, want: Tok) -> Result<(), ParseError> {
        if *self.peek() == want {
            self.next();
            Ok(())
        } else {
            err(
                self.pos(),
                format!("expected {want}, found {}", self.peek()),
            )
        }
    }

    fn ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.next();
                Ok(s)
            }
            t => err(self.pos(), format!("expected {what}, found {t}")),
        }
    }

    fn eof(&self) -> Result<(), ParseError> {
        match self.peek() {
            Tok::Eof => Ok(()),
            t => err(self.pos(), format!("unexpected {t} after end of item")),
        }
    }

    fn item(&mut self) -> Result<Item, ParseError> {
        if *self.peek() == Tok::DeclKw {
            self.decl().map(Item::Decl)
        } else {
            self.clause()
        }
    }

    fn annotations(&mut self) -> Result<Vec<String>, ParseError> {
        let mut out = Vec::new();
        while *self.peek() == Tok::At {
            self.next();
            out.push(self.ident("annotation name")?);
        }
        Ok(out)
    }

    fn decl(&mut self) -> Result<Decl, ParseError> {
        self.expect(Tok::DeclKw)?;
        let name = self.ident("predicate name")?;
        self.expect(Tok::LParen)?;
        let mut params = Vec::new();
        if *self.peek() != Tok::RParen {
            loop {
                let p = self.ident("parameter name")?;
                self.expect(Tok::Colon)?;
                let tp = self.pos();
                let t = self.ident("type")?;
                let ty = Type::parse(&t).map_or_else(
                    || {
                        err(
                            tp,
                            format!("unknown type `{t}` (expected int, string, symbol, or term)"),
                        )
                    },
                    Ok,
                )?;
                params.push((p, ty));
                if *self.peek() == Tok::Comma {
                    self.next();
                } else {
                    break;
                }
            }
        }
        let close = self.pos();
        self.expect(Tok::RParen)?;
        // Annotations belong to the declaration only on the same line, so an
        // annotated rule on the next line isn't swallowed.
        let mut annotations = Vec::new();
        while *self.peek() == Tok::At && self.pos().line == close.line {
            self.next();
            annotations.push(self.ident("annotation name")?);
        }
        if *self.peek() == Tok::Dot {
            self.next();
        }
        Ok(Decl {
            name,
            params,
            annotations,
        })
    }

    fn clause(&mut self) -> Result<Item, ParseError> {
        self.wildcards = 0;
        let start = self.pos();
        let annotations = self.annotations()?;
        let name = if matches!(self.peek(), Tok::Ident(_)) && *self.peek_at(1) == Tok::Colon {
            let n = self.ident("rule name")?;
            self.next();
            Some(n)
        } else {
            None
        };
        let head = self.head()?;
        let body = if *self.peek() == Tok::ColonDash {
            self.next();
            let mut body = vec![self.literal()?];
            while *self.peek() == Tok::Comma {
                self.next();
                body.push(self.literal()?);
            }
            Some(body)
        } else {
            None
        };
        self.expect(Tok::Dot)?;

        if body.is_none() && name.is_none() && annotations.is_empty() {
            let atom = head
                .as_atom()
                .map_or_else(|| err(start, "facts cannot contain aggregates"), Ok)?;
            let args = atom
                .args
                .iter()
                .map(Term::ground)
                .collect::<Option<Vec<_>>>()
                .map_or_else(
                    || {
                        err(
                            start,
                            format!("fact `{atom}` must be ground (no variables)"),
                        )
                    },
                    Ok,
                )?;
            return Ok(Item::Fact(Fact {
                pred: atom.pred,
                args,
            }));
        }
        Ok(Item::Rule(Rule {
            name: name.unwrap_or_default(),
            annotations,
            head,
            body: body.unwrap_or_default(),
        }))
    }

    fn head(&mut self) -> Result<Head, ParseError> {
        let pred = self.ident("predicate name")?;
        let mut args = Vec::new();
        if *self.peek() == Tok::LParen {
            self.next();
            loop {
                args.push(self.head_arg()?);
                if *self.peek() == Tok::Comma {
                    self.next();
                } else {
                    break;
                }
            }
            self.expect(Tok::RParen)?;
        }
        Ok(Head { pred, args })
    }

    fn head_arg(&mut self) -> Result<HeadArg, ParseError> {
        if let Tok::Ident(s) = self.peek().clone()
            && let Some(f) = AggFn::parse(&s)
            && *self.peek_at(1) == Tok::Cmp(CmpOp::Lt)
        {
            self.next();
            self.next();
            let v = match self.next() {
                Tok::Var(v) if v != "_" => v,
                t => {
                    return err(
                        self.pos(),
                        format!("expected variable in aggregate, found {t}"),
                    );
                }
            };
            self.expect(Tok::Cmp(CmpOp::Gt))?;
            return Ok(HeadArg::Agg(f, v));
        }
        Ok(HeadArg::Term(self.term()?))
    }

    fn literal(&mut self) -> Result<Literal, ParseError> {
        if *self.peek() == Tok::Ident("not".into()) && matches!(self.peek_at(1), Tok::Ident(_)) {
            self.next();
            return Ok(Literal::Neg(self.atom()?));
        }
        let p = self.pos();
        let t = self.term()?;
        if let Tok::Cmp(op) = *self.peek() {
            self.next();
            let r = self.term()?;
            return Ok(Literal::Cmp(t, op, r));
        }
        Atom::from_term(&t).map(Literal::Pos).map_or_else(
            || err(p, format!("expected an atom or comparison, found `{t}`")),
            Ok,
        )
    }

    fn atom(&mut self) -> Result<Atom, ParseError> {
        let p = self.pos();
        let t = self.term()?;
        Atom::from_term(&t).map_or_else(|| err(p, format!("expected an atom, found `{t}`")), Ok)
    }

    fn term(&mut self) -> Result<Term, ParseError> {
        let p = self.pos();
        match self.next() {
            Tok::Var(v) if v == "_" => {
                let n = self.wildcards;
                self.wildcards += 1;
                Ok(Term::Var(format!("_#{n}")))
            }
            Tok::Var(v) => Ok(Term::Var(v)),
            Tok::Int(n) => Ok(Term::Const(Value::Int(n))),
            Tok::Str(s) => Ok(Term::Const(Value::Str(s))),
            Tok::Ident(name) => {
                if *self.peek() != Tok::LParen {
                    return Ok(Term::Const(Value::Sym(name)));
                }
                self.next();
                let mut args = vec![self.term()?];
                while *self.peek() == Tok::Comma {
                    self.next();
                    args.push(self.term()?);
                }
                self.expect(Tok::RParen)?;
                Ok(Term::Compound(name, args))
            }
            t => err(p, format!("expected a term, found {t}")),
        }
    }
}

/// Parse a whole program (declarations, rules, facts).
pub fn parse_program(src: &str) -> Result<Vec<Item>, ParseError> {
    let mut p = Parser::new(src)?;
    let mut items = Vec::new();
    while *p.peek() != Tok::Eof {
        items.push(p.item()?);
    }
    Ok(items)
}

/// Parse exactly one declaration.
pub fn parse_decl(src: &str) -> Result<Decl, ParseError> {
    let mut p = Parser::new(src)?;
    let d = p.decl()?;
    p.eof()?;
    Ok(d)
}

/// Parse exactly one rule. A trailing `.` is optional. The name is empty if
/// the source didn't give one.
pub fn parse_rule(src: &str) -> Result<Rule, ParseError> {
    let trimmed = src.trim_end();
    let owned;
    let src = if trimmed.ends_with('.') {
        trimmed
    } else {
        owned = format!("{trimmed}.");
        &owned
    };
    let mut p = Parser::new(src)?;
    p.wildcards = 0;
    let annotations = p.annotations()?;
    let name = if matches!(p.peek(), Tok::Ident(_)) && *p.peek_at(1) == Tok::Colon {
        let n = p.ident("rule name")?;
        p.next();
        n
    } else {
        String::new()
    };
    let head = p.head()?;
    let mut body = Vec::new();
    if *p.peek() == Tok::ColonDash {
        p.next();
        body.push(p.literal()?);
        while *p.peek() == Tok::Comma {
            p.next();
            body.push(p.literal()?);
        }
    }
    p.expect(Tok::Dot)?;
    p.eof()?;
    Ok(Rule {
        name,
        annotations,
        head,
        body,
    })
}

/// Parse a single term, which may contain variables. A trailing `.` is allowed.
pub fn parse_term(src: &str) -> Result<Term, ParseError> {
    let mut p = Parser::new(src)?;
    let t = p.term()?;
    if *p.peek() == Tok::Dot {
        p.next();
    }
    p.eof()?;
    Ok(t)
}

/// Parse a ground value such as `handler(get, "/posts")`.
pub fn parse_value(src: &str) -> Result<Value, ParseError> {
    let t = parse_term(src)?;
    t.ground().ok_or(ParseError {
        line: 1,
        col: 1,
        msg: format!("`{t}` must be ground (no variables)"),
    })
}

/// Parse a ground atom as a fact.
pub fn parse_fact(src: &str) -> Result<Fact, ParseError> {
    let v = parse_value(src)?;
    Fact::from_value(&v).ok_or(ParseError {
        line: 1,
        col: 1,
        msg: format!("`{v}` is not an atom"),
    })
}

/// Parse an atom pattern (variables allowed), e.g. for queries.
pub fn parse_atom(src: &str) -> Result<Atom, ParseError> {
    let t = parse_term(src)?;
    Atom::from_term(&t).ok_or(ParseError {
        line: 1,
        col: 1,
        msg: format!("`{t}` is not an atom"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let src = r#"
            .decl handler(method: symbol, path: string) @requirement
            % a comment
            @requirement crud: resource(R) :- collection_path(R, P), handler(get, P), not banned(R, _), P != "x".
            total(D, count<C>) :- child(D, C).
            handler(get, "/po\"sts").
        "#;
        let items = parse_program(src).unwrap();
        assert_eq!(items.len(), 4);
        for item in &items {
            let text = match item {
                Item::Decl(d) => d.to_string(),
                Item::Rule(r) if !r.name.is_empty() => r.to_string(),
                Item::Rule(r) => format!("x: {}", r.to_string().split_once(": ").unwrap().1),
                Item::Fact(f) => format!("{f}."),
            };
            let again = parse_program(&text).unwrap();
            match (item, &again[0]) {
                (Item::Rule(a), Item::Rule(b)) if a.name.is_empty() => assert_eq!(a.body, b.body),
                (a, b) => assert_eq!(a, b),
            }
        }
    }

    #[test]
    fn reports_positions() {
        let e = parse_program("p(X) :- q(X)\nr(1).").unwrap_err();
        assert_eq!((e.line, e.col), (2, 1));
    }

    #[test]
    fn non_ground_fact_is_an_error() {
        assert!(parse_program("p(X).").is_err());
    }
}
