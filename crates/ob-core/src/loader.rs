//! Resolves `include` directives across files, applies `pushtag`/`poptag`
//! lexical scoping, and lowers the syntax-level AST into the semantic model
//! in `crate::model` (parsing dates, interning accounts/currencies,
//! validating account roots). Booking (inventory tracking, interpolation,
//! balance assertions) happens afterwards in `crate::booking`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use ob_diagnostics::SourceFile;
use ob_syntax::{ParseError, ast};
use ustr::Ustr;

use crate::error::BookingError;
use crate::model::*;

struct RawEntry {
    directive: ast::Directive,
    loc: Loc,
    extra_tags: Vec<String>,
}

#[derive(Default)]
struct LoadCtx {
    parse_errors: Vec<ParseError>,
    booking_errors: Vec<BookingError>,
    raw: Vec<RawEntry>,
    meta: LedgerMeta,
    tag_stack: Vec<String>,
    visited: HashSet<PathBuf>,
}

pub struct LoadOutput {
    pub directives: Vec<Directive>,
    pub meta: LedgerMeta,
    pub parse_errors: Vec<ParseError>,
    pub booking_errors: Vec<BookingError>,
}

/// Loads `root_path` and every file it (transitively) `include`s, and lowers
/// everything into date-sorted, semantically valid directives. Does not yet
/// book transactions — call `crate::booking::book` on the result for that.
pub fn load(root_path: &Path) -> LoadOutput {
    let mut ctx = LoadCtx::default();
    load_file(root_path, &mut ctx);

    let mut lowered = Vec::with_capacity(ctx.raw.len());
    for entry in ctx.raw {
        match lower_directive(entry.directive, &entry.loc, &entry.extra_tags) {
            Ok(d) => lowered.push(d),
            Err(e) => ctx.booking_errors.push(e),
        }
    }
    // Stable sort: entries with the same date keep the relative order they
    // were encountered in during the include-resolved file traversal, which
    // is what `lowered` already reflects.
    lowered.sort_by_key(Directive::date);

    LoadOutput {
        directives: lowered,
        meta: ctx.meta,
        parse_errors: ctx.parse_errors,
        booking_errors: ctx.booking_errors,
    }
}

fn load_file(path: &Path, ctx: &mut LoadCtx) {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !ctx.visited.insert(canonical) {
        return; // already included (or a cycle) — splice each file only once
    }

    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            ctx.booking_errors.push(BookingError::io(path, &e));
            return;
        }
    };
    let source = SourceFile::new(path.display().to_string(), contents);
    let (parsed, errors) = ob_syntax::parse(&source);
    ctx.parse_errors.extend(errors);

    for sp in parsed.directives {
        let loc = Loc {
            source: source.clone(),
            span: sp.span,
        };
        match sp.value {
            ast::Directive::Include { path: inc } => {
                let base = path.parent().unwrap_or_else(|| Path::new("."));
                load_file(&base.join(inc), ctx);
            }
            ast::Directive::PushTag(tag) => ctx.tag_stack.push(tag),
            ast::Directive::PopTag(tag) => {
                if let Some(pos) = ctx.tag_stack.iter().rposition(|t| *t == tag) {
                    ctx.tag_stack.remove(pos);
                }
            }
            ast::Directive::Option { name, value } => {
                match name.as_str() {
                    "title" => ctx.meta.title = Some(value.clone()),
                    "operating_currency" => ctx.meta.operating_currency = Some(value.clone()),
                    _ => {}
                }
                ctx.meta.options.push((name, value));
            }
            ast::Directive::Plugin { .. } => {
                // Beancount plugins execute arbitrary Python; out of scope
                // for a from-scratch Rust engine. Parsed and ignored.
            }
            ast::Directive::Query { .. } => {
                // Ad-hoc BQL queries aren't modeled yet (later CLI feature).
            }
            other => ctx.raw.push(RawEntry {
                directive: other,
                loc,
                extra_tags: ctx.tag_stack.clone(),
            }),
        }
    }
}

fn parse_date(loc: &Loc, text: &str) -> Result<NaiveDate, BookingError> {
    NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| BookingError::invalid_date(loc, text))
}

fn lower_account(loc: &Loc, a: ast::Account) -> Result<Account, BookingError> {
    let full = a.segments.join(":");
    let account = Account::new(&full);
    if account.root_type().is_none() {
        return Err(BookingError::unknown_account_root(loc, &full));
    }
    Ok(account)
}

fn lower_amount(a: ast::Amount) -> Amount {
    Amount {
        number: a.number,
        currency: Ustr::from(&a.currency),
    }
}

fn lower_custom_value(loc: &Loc, v: ast::CustomValue) -> Result<CustomValue, BookingError> {
    Ok(match v {
        ast::CustomValue::Str(s) => CustomValue::Str(s),
        ast::CustomValue::Date(d) => CustomValue::Date(parse_date(loc, &d)?),
        ast::CustomValue::Amount(a) => CustomValue::Amount(lower_amount(a)),
        ast::CustomValue::Account(a) => CustomValue::Account(lower_account(loc, a)?),
        ast::CustomValue::Currency(c) => CustomValue::Currency(Ustr::from(&c)),
        ast::CustomValue::Bool(b) => CustomValue::Bool(b),
    })
}

fn lower_transaction(
    txn: ast::Transaction,
    loc: &Loc,
    extra_tags: &[String],
) -> Result<Transaction, BookingError> {
    let date = parse_date(loc, &txn.date)?;
    let mut postings = Vec::with_capacity(txn.postings.len());
    for sp in txn.postings {
        let p = sp.value;
        let account = lower_account(loc, p.account)?;
        postings.push(Posting {
            account,
            amount: p.amount.map(lower_amount),
            flag: p.flag,
            meta: p.meta.into_iter().map(|m| (m.key, m.raw_value)).collect(),
            loc: Loc {
                source: loc.source.clone(),
                span: sp.span,
            },
        });
    }
    let mut tags = txn.tags;
    for t in extra_tags {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }
    Ok(Transaction {
        date,
        flag: txn.flag,
        payee: txn.payee,
        narration: txn.narration,
        tags,
        links: txn.links,
        meta: txn.meta.into_iter().map(|m| (m.key, m.raw_value)).collect(),
        postings,
        loc: loc.clone(),
    })
}

fn lower_directive(
    d: ast::Directive,
    loc: &Loc,
    extra_tags: &[String],
) -> Result<Directive, BookingError> {
    use ast::Directive as A;
    Ok(match d {
        A::Open {
            date,
            account,
            currencies,
            ..
        } => Directive::Open(Open {
            date: parse_date(loc, &date)?,
            account: lower_account(loc, account)?,
            currencies: currencies.iter().map(|c| Ustr::from(c.as_str())).collect(),
            loc: loc.clone(),
        }),
        A::Close { date, account } => Directive::Close(Close {
            date: parse_date(loc, &date)?,
            account: lower_account(loc, account)?,
            loc: loc.clone(),
        }),
        A::Balance {
            date,
            account,
            amount,
        } => Directive::Balance(Balance {
            date: parse_date(loc, &date)?,
            account: lower_account(loc, account)?,
            amount: lower_amount(amount),
            loc: loc.clone(),
        }),
        A::Pad {
            date,
            account,
            source_account,
        } => Directive::Pad(Pad {
            date: parse_date(loc, &date)?,
            account: lower_account(loc, account)?,
            source_account: lower_account(loc, source_account)?,
            loc: loc.clone(),
        }),
        A::Price {
            date,
            currency,
            amount,
        } => Directive::Price(Price {
            date: parse_date(loc, &date)?,
            currency: Ustr::from(currency.as_str()),
            amount: lower_amount(amount),
            loc: loc.clone(),
        }),
        A::Event { date, name, value } => Directive::Event(Event {
            date: parse_date(loc, &date)?,
            name,
            value,
            loc: loc.clone(),
        }),
        A::Note {
            date,
            account,
            comment,
        } => Directive::Note(Note {
            date: parse_date(loc, &date)?,
            account: lower_account(loc, account)?,
            comment,
            loc: loc.clone(),
        }),
        A::Document {
            date,
            account,
            path,
        } => Directive::Document(Document {
            date: parse_date(loc, &date)?,
            account: lower_account(loc, account)?,
            path,
            loc: loc.clone(),
        }),
        A::Commodity { date, currency, .. } => Directive::Commodity(Commodity {
            date: parse_date(loc, &date)?,
            currency: Ustr::from(currency.as_str()),
            loc: loc.clone(),
        }),
        A::Custom { date, name, values } => Directive::Custom(Custom {
            date: parse_date(loc, &date)?,
            name,
            values: values
                .into_iter()
                .map(|v| lower_custom_value(loc, v))
                .collect::<Result<_, _>>()?,
            loc: loc.clone(),
        }),
        A::Transaction(txn) => Directive::Transaction(lower_transaction(txn, loc, extra_tags)?),
        A::Query { .. }
        | A::Option { .. }
        | A::Include { .. }
        | A::Plugin { .. }
        | A::PushTag(_)
        | A::PopTag(_) => {
            unreachable!("consumed during file traversal in `load_file`")
        }
    })
}
