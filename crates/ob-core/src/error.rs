use std::sync::Arc;

use ob_diagnostics::{Diagnostic, Error, NamedSource};

use crate::model::Loc;

/// A single problem found while resolving `include`s or booking the sorted
/// directive stream. Like `ob_syntax::ParseError`, each carries its own
/// source snippet so a whole ledger's worth of errors can be reported at
/// once (via `BookingErrors`) with every location shown independently.
#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
pub struct BookingError {
    pub message: String,
    #[source_code]
    pub src: NamedSource<Arc<str>>,
    #[label("{label_text}")]
    pub span: miette::SourceSpan,
    pub label_text: String,
    #[help]
    pub help_text: Option<String>,
}

impl BookingError {
    pub fn new(loc: &Loc, message: impl Into<String>, label_text: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            src: loc.source.named_source(),
            span: loc.span.into(),
            label_text: label_text.into(),
            help_text: None,
        }
    }

    pub fn with_help(mut self, help_text: impl Into<String>) -> Self {
        self.help_text = Some(help_text.into());
        self
    }

    pub fn io(path: &std::path::Path, err: &std::io::Error) -> Self {
        Self {
            message: format!("couldn't read `{}`: {err}", path.display()),
            src: NamedSource::new(path.display().to_string(), Arc::from("")),
            span: (0, 0).into(),
            label_text: String::new(),
            help_text: None,
        }
    }

    pub fn invalid_date(loc: &Loc, text: &str) -> Self {
        Self::new(
            loc,
            format!("invalid date `{text}`"),
            "not a valid calendar date",
        )
    }

    pub fn unknown_account_root(loc: &Loc, account: &str) -> Self {
        Self::new(
            loc,
            format!("`{account}` doesn't start with a valid root account"),
            "unknown root",
        )
        .with_help("accounts must start with Assets, Liabilities, Equity, Income, or Expenses")
    }

    pub fn account_reopened(loc: &Loc, account: &str, first_opened: chrono::NaiveDate) -> Self {
        Self::new(
            loc,
            format!("`{account}` is already open"),
            format!("first opened on {first_opened}"),
        )
    }

    pub fn account_already_closed(loc: &Loc, account: &str) -> Self {
        Self::new(
            loc,
            format!("`{account}` is already closed"),
            "duplicate close",
        )
    }

    pub fn account_not_open(loc: &Loc, account: &str, date: chrono::NaiveDate) -> Self {
        Self::new(
            loc,
            format!("`{account}` is not open on {date}"),
            "used here before being opened (or after being closed)",
        )
    }

    pub fn multiple_elided_postings(loc: &Loc) -> Self {
        Self::new(
            loc,
            "a transaction can have at most one posting with an elided amount",
            "second elided posting here",
        )
    }

    pub fn mixed_currency_elision(loc: &Loc) -> Self {
        Self::new(
            loc,
            "can't infer this posting's amount: the other postings use more than one currency",
            "amount needed here",
        )
        .with_help("write the amount explicitly, or add a `@price`/`{cost}` to convert currencies")
    }

    pub fn does_not_balance(loc: &Loc, currency: &str, residual: rust_decimal::Decimal) -> Self {
        Self::new(
            loc,
            format!("transaction does not balance: {residual} {currency} left over"),
            "postings summed here",
        )
    }

    pub fn balance_mismatch(
        loc: &Loc,
        account: &str,
        expected: rust_decimal::Decimal,
        actual: rust_decimal::Decimal,
        currency: &str,
    ) -> Self {
        Self::new(
            loc,
            format!(
                "balance assertion failed for `{account}`: expected {expected} {currency}, got {actual} {currency}"
            ),
            "asserted here",
        )
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("{count} error(s) while loading the ledger", count = errors.len())]
pub struct BookingErrors {
    #[related]
    pub errors: Vec<BookingError>,
}
