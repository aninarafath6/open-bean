//! Shared span and diagnostic-rendering types used by every open-bean crate,
//! so `ob-syntax` parse errors and `ob-core` booking errors render through the
//! same miette machinery on both the CLI and the web API.

pub use miette::{
    Diagnostic, GraphicalReportHandler, JSONReportHandler, NamedSource, NarratableReportHandler,
    Report,
};
pub use thiserror::Error;

/// A byte-offset span into a source file, independent of miette so `ob-syntax`
/// can construct spans while lexing without depending on error-rendering types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub offset: usize,
    pub len: usize,
}

impl Span {
    pub fn new(offset: usize, len: usize) -> Self {
        Self { offset, len }
    }

    pub fn from_range(range: std::ops::Range<usize>) -> Self {
        Self {
            offset: range.start,
            len: range.end.saturating_sub(range.start),
        }
    }

    /// Smallest span covering both `self` and `other`.
    pub fn merge(self, other: Span) -> Span {
        let start = self.offset.min(other.offset);
        let end = (self.offset + self.len).max(other.offset + other.len);
        Span::from_range(start..end)
    }
}

impl From<Span> for miette::SourceSpan {
    fn from(span: Span) -> Self {
        (span.offset, span.len).into()
    }
}

/// A source file loaded once, shared by every diagnostic raised against it.
/// Cloning is cheap: `path` and `contents` are reference-counted.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: std::sync::Arc<str>,
    pub contents: std::sync::Arc<str>,
}

impl SourceFile {
    pub fn new(
        path: impl Into<std::sync::Arc<str>>,
        contents: impl Into<std::sync::Arc<str>>,
    ) -> Self {
        Self {
            path: path.into(),
            contents: contents.into(),
        }
    }

    pub fn named_source(&self) -> NamedSource<std::sync::Arc<str>> {
        NamedSource::new(&*self.path, self.contents.clone())
    }
}

/// Render a diagnostic (and its source-code chain) to a plain string using the
/// graphical (CLI-friendly, ANSI-colored) handler. Used by `ob-cli`.
pub fn render_graphical(report: &Report) -> String {
    let mut out = String::new();
    GraphicalReportHandler::new()
        .render_report(&mut out, report.as_ref())
        .expect("formatting a diagnostic never fails");
    out
}

/// Render a diagnostic as plain, ANSI-free prose — safe to drop straight
/// into an HTML `<pre>` (after escaping) since it never emits terminal
/// color codes. Used by the web UI.
pub fn render_narratable(report: &Report) -> String {
    let mut out = String::new();
    NarratableReportHandler::new()
        .render_report(&mut out, report.as_ref())
        .expect("formatting a diagnostic never fails");
    out
}
