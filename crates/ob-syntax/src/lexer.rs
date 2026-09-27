//! Tokenizes a whole source file with `logos`, then groups the flat token
//! stream into physical lines and, from those, into blocks: a line starting
//! at column 0 opens a new directive, and every following line indented past
//! column 0 is a continuation (a posting or a metadata line) of it. This is
//! exactly how real beancount decides where one directive ends and the next
//! begins.

use logos::Logos;
use ob_diagnostics::Span;

fn unescape(raw: &str) -> String {
    // `raw` includes the surrounding quotes.
    let inner = &raw[1..raw.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Logos, Debug, Clone, PartialEq)]
#[logos(skip r"[ \t\r]+")]
#[logos(skip r";[^\n]*")]
pub enum Token {
    #[token("\n")]
    Newline,

    #[regex(r#""([^"\\]|\\.)*""#, |lex| unescape(lex.slice()))]
    Str(String),

    #[regex(r"\d{4}[-/]\d{2}[-/]\d{2}", |lex| lex.slice().to_string())]
    Date(String),

    #[regex(r"[+-]?[0-9][0-9,]*(\.[0-9]+)?", |lex| lex.slice().to_string())]
    Number(String),

    // Must be tried before `Currency`/`Ident`: requires at least one `:`.
    #[regex(r"[A-Za-z][A-Za-z0-9\-]*(:[A-Za-z0-9\-]+)+", |lex| lex.slice().to_string(), priority = 3)]
    Account(String),

    #[regex(r"[A-Z][A-Z0-9'\._\-]{0,22}", |lex| lex.slice().to_string())]
    Currency(String),

    #[regex(r"[a-z][a-zA-Z0-9_]*", |lex| lex.slice().to_string())]
    Ident(String),

    #[regex(r"#[A-Za-z0-9\-_/.]+", |lex| lex.slice()[1..].to_string())]
    Tag(String),

    #[regex(r"\^[A-Za-z0-9\-_/.]+", |lex| lex.slice()[1..].to_string())]
    Link(String),

    #[token("*")]
    Star,
    #[token("!")]
    Bang,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("@@")]
    AtAt,
    #[token("@")]
    At,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
}

impl Token {
    /// Human-readable name for diagnostics (`expected X, found <name>`).
    pub fn describe(&self) -> String {
        match self {
            Token::Newline => "end of line".into(),
            Token::Str(s) => format!("string \"{s}\""),
            Token::Date(d) => format!("date {d}"),
            Token::Number(n) => format!("number {n}"),
            Token::Account(a) => format!("account {a}"),
            Token::Currency(c) => format!("currency {c}"),
            Token::Ident(i) => format!("`{i}`"),
            Token::Tag(t) => format!("tag #{t}"),
            Token::Link(l) => format!("link ^{l}"),
            Token::Star => "`*`".into(),
            Token::Bang => "`!`".into(),
            Token::LBrace => "`{`".into(),
            Token::RBrace => "`}`".into(),
            Token::AtAt => "`@@`".into(),
            Token::At => "`@`".into(),
            Token::Comma => "`,`".into(),
            Token::Colon => "`:`".into(),
        }
    }
}

/// One non-blank, non-comment-only source line: its tokens plus the column
/// (byte offset from the start of the line) its first token starts at.
#[derive(Debug, Clone)]
pub struct PhysicalLine {
    pub indent: usize,
    pub tokens: Vec<(Token, Span)>,
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn column_of(line_starts: &[usize], offset: usize) -> usize {
    let line_start = match line_starts.binary_search(&offset) {
        Ok(i) => line_starts[i],
        Err(i) => line_starts[i - 1],
    };
    offset - line_start
}

pub fn tokenize_lines(
    source: &ob_diagnostics::SourceFile,
    errors: &mut Vec<crate::error::ParseError>,
) -> Vec<PhysicalLine> {
    let starts = line_starts(&source.contents);
    let mut lexer = Token::lexer(&source.contents);
    let mut lines = Vec::new();
    let mut current: Vec<(Token, Span)> = Vec::new();

    while let Some(result) = lexer.next() {
        let range = lexer.span();
        match result {
            Ok(Token::Newline) => {
                if !current.is_empty() {
                    lines.push(finish_line(std::mem::take(&mut current), &starts));
                }
            }
            Ok(tok) => current.push((tok, Span::from_range(range))),
            Err(()) => {
                errors.push(crate::error::ParseError::lex_error(
                    source,
                    Span::from_range(range),
                ));
            }
        }
    }
    if !current.is_empty() {
        lines.push(finish_line(current, &starts));
    }
    lines
}

fn finish_line(tokens: Vec<(Token, Span)>, starts: &[usize]) -> PhysicalLine {
    let indent = column_of(starts, tokens[0].1.offset);
    PhysicalLine { indent, tokens }
}
