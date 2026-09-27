//! The open-bean ledger engine: loads a beancount-syntax file (following
//! `include`s), books it (interpolating elided postings, checking that
//! every transaction balances and every `balance` assertion holds), and
//! exposes a small query API for reports to build on.

// See the matching allow in `ob_syntax`: `BookingError` carries a full
// source snippet so errors render independently, which makes it bigger
// than clippy's default threshold; boxing every fallible booking function
// isn't worth it for a path that's almost never taken.
#![allow(clippy::result_large_err)]

pub mod booking;
pub mod error;
mod loader;
pub mod model;
pub mod query;

pub use error::{BookingError, BookingErrors};
pub use model::*;
pub use ob_syntax::ParseError;

use std::path::Path;

/// Everything that can go wrong loading and booking a ledger, plus the
/// ledger itself (which is still populated best-effort even when some
/// directives failed to book, so a web UI can show *what* is wrong without
/// refusing to show anything at all).
pub struct LoadResult {
    pub ledger: Ledger,
    pub parse_errors: Vec<ParseError>,
    pub booking_errors: Vec<BookingError>,
}

impl LoadResult {
    pub fn is_ok(&self) -> bool {
        self.parse_errors.is_empty() && self.booking_errors.is_empty()
    }
}

/// Loads and books `root_path` (and everything it transitively `include`s).
pub fn load(root_path: &Path) -> LoadResult {
    let loaded = loader::load(root_path);
    let output = booking::book(loaded.directives, loaded.meta);
    let mut booking_errors = loaded.booking_errors;
    booking_errors.extend(output.errors);
    LoadResult {
        ledger: output.ledger,
        parse_errors: loaded.parse_errors,
        booking_errors,
    }
}
