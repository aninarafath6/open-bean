//! Beancount-syntax lexer and parser. Turns a source file into a
//! source-spanned [`Ledger`] AST plus any [`ParseError`]s found — with no
//! knowledge of accounting semantics (account types, arithmetic, booking).
//! That belongs to `ob-core`, which consumes this crate's output.

// `ParseError` carries a full `NamedSource` snippet so every error can be
// rendered independently (see `ParseErrors`'s doc comment); that makes it
// larger than clippy's default threshold. Errors are the rare path and
// already heap-collected into a `Vec`, so boxing every fallible parser
// function to shrink a `Result` that's almost never `Err` isn't worth the
// indirection.
#![allow(clippy::result_large_err)]

pub mod ast;
pub mod error;
pub mod lexer;
mod parser;

pub use ast::*;
pub use error::{ParseError, ParseErrors};

use ob_diagnostics::SourceFile;

/// Parses one source file, collecting every error it can find rather than
/// stopping at the first one. Multiple parse errors in the same file are
/// common while editing, so callers get the full list at once.
pub fn parse(source: &SourceFile) -> (Ledger, Vec<ParseError>) {
    parser::parse(source)
}
