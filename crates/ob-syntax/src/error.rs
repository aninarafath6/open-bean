use std::sync::Arc;

use ob_diagnostics::{Diagnostic, Error, NamedSource, SourceFile};

/// A single parse problem, already bound to the source file it came from so
/// it renders a full snippet on its own — useful because a parse run
/// collects every error in a file (via `ParseErrors`) instead of stopping at
/// the first one, and each needs to show its own location independently.
#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
pub struct ParseError {
    pub message: String,
    #[source_code]
    pub src: NamedSource<Arc<str>>,
    #[label("{label_text}")]
    pub span: miette::SourceSpan,
    pub label_text: String,
    #[help]
    pub help_text: Option<String>,
}

impl ParseError {
    pub fn new(
        source: &SourceFile,
        span: ob_diagnostics::Span,
        message: impl Into<String>,
        label_text: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            src: source.named_source(),
            span: span.into(),
            label_text: label_text.into(),
            help_text: None,
        }
    }

    pub fn with_help(mut self, help_text: impl Into<String>) -> Self {
        self.help_text = Some(help_text.into());
        self
    }

    pub fn unexpected_token(
        source: &SourceFile,
        span: ob_diagnostics::Span,
        expected: &str,
        found: &str,
    ) -> Self {
        Self::new(
            source,
            span,
            format!("expected {expected}, found {found}"),
            "here",
        )
    }

    pub fn unexpected_eof(source: &SourceFile, span: ob_diagnostics::Span, expected: &str) -> Self {
        Self::new(
            source,
            span,
            format!("unexpected end of directive, expected {expected}"),
            "directive ends here",
        )
    }

    pub fn unexpected_indent(source: &SourceFile, span: ob_diagnostics::Span) -> Self {
        Self::new(
            source,
            span,
            "line is indented but no directive precedes it",
            "unexpected indentation",
        )
        .with_help("indented lines attach to the directive on the previous non-indented line")
    }

    pub fn invalid_number(source: &SourceFile, span: ob_diagnostics::Span, text: &str) -> Self {
        Self::new(
            source,
            span,
            format!("invalid number literal `{text}`"),
            "could not parse as a decimal",
        )
    }

    pub fn unknown_directive(
        source: &SourceFile,
        span: ob_diagnostics::Span,
        keyword: &str,
    ) -> Self {
        Self::new(
            source,
            span,
            format!("unknown directive keyword `{keyword}`"),
            "not a recognized directive",
        )
    }

    pub fn lex_error(source: &SourceFile, span: ob_diagnostics::Span) -> Self {
        Self::new(
            source,
            span,
            "unrecognized token",
            "this doesn't match any known token",
        )
    }
}

/// Every error collected from one parse run.
#[derive(Debug, Error, Diagnostic)]
#[error("{count} error(s) while parsing", count = errors.len())]
pub struct ParseErrors {
    #[related]
    pub errors: Vec<ParseError>,
}

impl ParseErrors {
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }
}
