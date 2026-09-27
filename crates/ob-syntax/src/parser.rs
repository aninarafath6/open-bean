use ob_diagnostics::{SourceFile, Span};
use rust_decimal::Decimal;

use crate::ast::*;
use crate::error::ParseError;
use crate::lexer::{PhysicalLine, Token, tokenize_lines};

struct Block {
    header: PhysicalLine,
    sub_lines: Vec<PhysicalLine>,
}

fn group_blocks(
    lines: Vec<PhysicalLine>,
    source: &SourceFile,
    errors: &mut Vec<ParseError>,
) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut iter = lines.into_iter().peekable();
    while let Some(line) = iter.next() {
        if line.indent != 0 {
            errors.push(ParseError::unexpected_indent(source, line.tokens[0].1));
            continue;
        }
        let mut sub_lines = Vec::new();
        while let Some(next) = iter.peek() {
            if next.indent == 0 {
                break;
            }
            sub_lines.push(iter.next().expect("peeked"));
        }
        blocks.push(Block {
            header: line,
            sub_lines,
        });
    }
    blocks
}

/// A cursor over one line's tokens. Parsing a single directive/posting/meta
/// line either fully succeeds or bails with one `ParseError`; the caller
/// decides whether to skip just that line or the whole block.
struct Cursor<'a> {
    tokens: &'a [(Token, Span)],
    pos: usize,
    source: &'a SourceFile,
}

impl<'a> Cursor<'a> {
    fn new(tokens: &'a [(Token, Span)], source: &'a SourceFile) -> Self {
        Self {
            tokens,
            pos: 0,
            source,
        }
    }

    fn eof_span(&self) -> Span {
        self.tokens
            .last()
            .map(|(_, s)| *s)
            .unwrap_or(Span::new(0, 0))
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn peek_span(&self) -> Option<Span> {
        self.tokens.get(self.pos).map(|(_, s)| *s)
    }

    fn bump(&mut self) -> Option<(Token, Span)> {
        let item = self.tokens.get(self.pos).cloned();
        if item.is_some() {
            self.pos += 1;
        }
        item
    }

    fn at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn expect<T>(
        &mut self,
        expected: &'static str,
        f: impl FnOnce(&Token) -> Option<T>,
    ) -> Result<T, ParseError> {
        match self.peek() {
            Some(tok) => match f(tok) {
                Some(v) => {
                    self.pos += 1;
                    Ok(v)
                }
                None => Err(ParseError::unexpected_token(
                    self.source,
                    self.peek_span().unwrap(),
                    expected,
                    &tok.describe(),
                )),
            },
            None => Err(ParseError::unexpected_eof(
                self.source,
                self.eof_span(),
                expected,
            )),
        }
    }

    fn expect_str(&mut self, expected: &'static str) -> Result<String, ParseError> {
        self.expect(expected, |t| match t {
            Token::Str(s) => Some(s.clone()),
            _ => None,
        })
    }

    fn expect_date(&mut self) -> Result<String, ParseError> {
        self.expect("a date (YYYY-MM-DD)", |t| match t {
            Token::Date(d) => Some(d.replace('/', "-")),
            _ => None,
        })
    }

    fn expect_account(&mut self) -> Result<Account, ParseError> {
        self.expect("an account", |t| match t {
            Token::Account(a) => Some(Account::from_colon_path(a)),
            _ => None,
        })
    }

    fn expect_currency(&mut self) -> Result<String, ParseError> {
        self.expect("a currency", |t| match t {
            Token::Currency(c) => Some(c.clone()),
            _ => None,
        })
    }

    fn expect_number(&mut self) -> Result<Decimal, ParseError> {
        let (text, span) = match self.peek() {
            Some(Token::Number(n)) => (n.clone(), self.peek_span().unwrap()),
            _ => {
                return self.expect("a number", |_| None::<Decimal>);
            }
        };
        self.pos += 1;
        let cleaned = text.replace(',', "");
        cleaned
            .parse::<Decimal>()
            .map_err(|_| ParseError::invalid_number(self.source, span, &text))
    }

    fn expect_amount(&mut self) -> Result<Amount, ParseError> {
        let number = self.expect_number()?;
        let currency = self.expect_currency()?;
        Ok(Amount { number, currency })
    }

    fn expect_tag(&mut self) -> Result<String, ParseError> {
        self.expect("a #tag", |t| match t {
            Token::Tag(s) => Some(s.clone()),
            _ => None,
        })
    }
}

pub fn parse(source: &SourceFile) -> (Ledger, Vec<ParseError>) {
    let mut errors = Vec::new();
    let lines = tokenize_lines(source, &mut errors);
    let blocks = group_blocks(lines, source, &mut errors);

    let mut directives = Vec::with_capacity(blocks.len());
    for block in blocks {
        match parse_block(&block, source, &mut errors) {
            Some(directive) => {
                let span = block
                    .header
                    .tokens
                    .first()
                    .map(|(_, s)| *s)
                    .unwrap_or(Span::new(0, 0));
                let span = block
                    .sub_lines
                    .last()
                    .and_then(|l| l.tokens.last())
                    .map(|(_, s)| span.merge(*s))
                    .unwrap_or(span);
                directives.push(Spanned::new(directive, span));
            }
            None => { /* error already recorded; skip this block */ }
        }
    }
    (Ledger { directives }, errors)
}

fn parse_block(
    block: &Block,
    source: &SourceFile,
    errors: &mut Vec<ParseError>,
) -> Option<Directive> {
    let mut cur = Cursor::new(&block.header.tokens, source);

    let first = cur.peek()?.clone();
    let directive = match first {
        Token::Date(_) => parse_dated_directive(&mut cur, block, source, errors),
        Token::Ident(word) => match word.as_str() {
            "option" => parse_option(&mut cur),
            "include" => parse_include(&mut cur),
            "plugin" => parse_plugin(&mut cur),
            "pushtag" => parse_pushtag(&mut cur),
            "poptag" => parse_poptag(&mut cur),
            _ => Err(ParseError::unknown_directive(
                source,
                cur.peek_span().unwrap(),
                &word,
            )),
        },
        _ => Err(ParseError::unexpected_token(
            source,
            cur.peek_span().unwrap(),
            "a date or a directive keyword",
            &first.describe(),
        )),
    };

    match directive {
        Ok(d) => Some(d),
        Err(e) => {
            errors.push(e);
            None
        }
    }
}

fn parse_dated_directive(
    cur: &mut Cursor,
    block: &Block,
    source: &SourceFile,
    errors: &mut Vec<ParseError>,
) -> Result<Directive, ParseError> {
    let date = cur.expect_date()?;
    match cur.peek().cloned() {
        Some(Token::Star) => {
            cur.bump();
            parse_transaction(cur, block, source, errors, date, '*')
        }
        Some(Token::Bang) => {
            cur.bump();
            parse_transaction(cur, block, source, errors, date, '!')
        }
        Some(Token::Ident(word)) if word == "txn" => {
            cur.bump();
            parse_transaction(cur, block, source, errors, date, '*')
        }
        Some(Token::Ident(word)) => {
            cur.bump();
            match word.as_str() {
                "open" => parse_open(cur, date),
                "close" => parse_close(cur, date),
                "balance" => parse_balance(cur, date),
                "pad" => parse_pad(cur, date),
                "price" => parse_price(cur, date),
                "event" => parse_event(cur, date),
                "note" => parse_note(cur, date),
                "document" => parse_document(cur, date),
                "query" => parse_query(cur, date),
                "custom" => parse_custom(cur, date),
                "commodity" => parse_commodity(cur, block, date, errors),
                _ => Err(ParseError::unknown_directive(
                    source,
                    cur.tokens[cur.pos - 1].1,
                    &word,
                )),
            }
        }
        Some(other) => Err(ParseError::unexpected_token(
            source,
            cur.peek_span().unwrap(),
            "a transaction flag or directive keyword",
            &other.describe(),
        )),
        None => Err(ParseError::unexpected_eof(
            source,
            cur.eof_span(),
            "a transaction flag or directive keyword",
        )),
    }
}

fn parse_open(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let account = cur.expect_account()?;
    let mut currencies = Vec::new();
    while let Some(Token::Currency(_)) = cur.peek() {
        currencies.push(cur.expect_currency()?);
        if matches!(cur.peek(), Some(Token::Comma)) {
            cur.bump();
        } else {
            break;
        }
    }
    let booking = if matches!(cur.peek(), Some(Token::Str(_))) {
        Some(cur.expect_str("a booking method")?)
    } else {
        None
    };
    Ok(Directive::Open {
        date,
        account,
        currencies,
        booking,
    })
}

fn parse_close(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let account = cur.expect_account()?;
    Ok(Directive::Close { date, account })
}

fn parse_balance(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let account = cur.expect_account()?;
    let amount = cur.expect_amount()?;
    Ok(Directive::Balance {
        date,
        account,
        amount,
    })
}

fn parse_pad(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let account = cur.expect_account()?;
    let source_account = cur.expect_account()?;
    Ok(Directive::Pad {
        date,
        account,
        source_account,
    })
}

fn parse_price(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let currency = cur.expect_currency()?;
    let amount = cur.expect_amount()?;
    Ok(Directive::Price {
        date,
        currency,
        amount,
    })
}

fn parse_event(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let name = cur.expect_str("the event name")?;
    let value = cur.expect_str("the event value")?;
    Ok(Directive::Event { date, name, value })
}

fn parse_note(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let account = cur.expect_account()?;
    let comment = cur.expect_str("the note text")?;
    Ok(Directive::Note {
        date,
        account,
        comment,
    })
}

fn parse_document(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let account = cur.expect_account()?;
    let path = cur.expect_str("a document path")?;
    Ok(Directive::Document {
        date,
        account,
        path,
    })
}

fn parse_query(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let name = cur.expect_str("the query name")?;
    let query_string = cur.expect_str("the query string")?;
    Ok(Directive::Query {
        date,
        name,
        query_string,
    })
}

fn parse_custom(cur: &mut Cursor, date: String) -> Result<Directive, ParseError> {
    let name = cur.expect_str("the custom directive name")?;
    let mut values = Vec::new();
    while !cur.at_end() {
        let value = match cur.peek().cloned().unwrap() {
            Token::Str(s) => {
                cur.bump();
                CustomValue::Str(s)
            }
            Token::Date(d) => {
                cur.bump();
                CustomValue::Date(d)
            }
            Token::Account(_) => CustomValue::Account(cur.expect_account()?),
            Token::Number(_) => CustomValue::Amount(cur.expect_amount()?),
            Token::Currency(c) if c == "TRUE" => {
                cur.bump();
                CustomValue::Bool(true)
            }
            Token::Currency(c) if c == "FALSE" => {
                cur.bump();
                CustomValue::Bool(false)
            }
            Token::Currency(_) => CustomValue::Currency(cur.expect_currency()?),
            other => {
                return Err(ParseError::unexpected_token(
                    cur.source,
                    cur.peek_span().unwrap(),
                    "a custom directive value",
                    &other.describe(),
                ));
            }
        };
        values.push(value);
    }
    Ok(Directive::Custom { date, name, values })
}

fn parse_commodity(
    cur: &mut Cursor,
    block: &Block,
    date: String,
    errors: &mut Vec<ParseError>,
) -> Result<Directive, ParseError> {
    let currency = cur.expect_currency()?;
    let meta = parse_meta_lines(&block.sub_lines, cur.source, errors);
    Ok(Directive::Commodity {
        date,
        currency,
        meta,
    })
}

fn parse_option(cur: &mut Cursor) -> Result<Directive, ParseError> {
    cur.bump(); // the `option` keyword itself
    let name = cur.expect_str("the option name")?;
    let value = cur.expect_str("the option value")?;
    Ok(Directive::Option { name, value })
}

fn parse_include(cur: &mut Cursor) -> Result<Directive, ParseError> {
    cur.bump();
    let path = cur.expect_str("a file path")?;
    Ok(Directive::Include { path })
}

fn parse_plugin(cur: &mut Cursor) -> Result<Directive, ParseError> {
    cur.bump();
    let module = cur.expect_str("a plugin module name")?;
    let config = if matches!(cur.peek(), Some(Token::Str(_))) {
        Some(cur.expect_str("plugin config")?)
    } else {
        None
    };
    Ok(Directive::Plugin { module, config })
}

fn parse_pushtag(cur: &mut Cursor) -> Result<Directive, ParseError> {
    cur.bump();
    let tag = cur.expect_tag()?;
    Ok(Directive::PushTag(tag))
}

fn parse_poptag(cur: &mut Cursor) -> Result<Directive, ParseError> {
    cur.bump();
    let tag = cur.expect_tag()?;
    Ok(Directive::PopTag(tag))
}

fn parse_transaction(
    cur: &mut Cursor,
    block: &Block,
    source: &SourceFile,
    errors: &mut Vec<ParseError>,
    date: String,
    flag: char,
) -> Result<Directive, ParseError> {
    let mut strings = Vec::new();
    while let Some(Token::Str(_)) = cur.peek() {
        strings.push(cur.expect_str("a string")?);
    }
    let (payee, narration) = match strings.len() {
        0 => (None, None),
        1 => (None, Some(strings.into_iter().next().unwrap())),
        _ => {
            let mut it = strings.into_iter();
            (it.next(), it.next())
        }
    };

    let mut tags = Vec::new();
    let mut links = Vec::new();
    loop {
        match cur.peek().cloned() {
            Some(Token::Tag(t)) => {
                cur.bump();
                tags.push(t);
            }
            Some(Token::Link(l)) => {
                cur.bump();
                links.push(l);
            }
            _ => break,
        }
    }

    let mut txn_meta = Vec::new();
    let mut postings = Vec::new();
    let mut current_posting_meta: Option<usize> = None;

    for line in &block.sub_lines {
        if is_meta_line(&line.tokens) {
            let meta = parse_meta_line(line, source);
            match current_posting_meta {
                Some(idx) => {
                    let posting: &mut Spanned<Posting> = &mut postings[idx];
                    posting.value.meta.push(meta);
                }
                None => txn_meta.push(meta),
            }
            continue;
        }
        match parse_posting(line, source) {
            Ok(posting) => {
                let span = line
                    .tokens
                    .first()
                    .unwrap()
                    .1
                    .merge(line.tokens.last().unwrap().1);
                postings.push(Spanned::new(posting, span));
                current_posting_meta = Some(postings.len() - 1);
            }
            Err(e) => errors.push(e),
        }
    }

    Ok(Directive::Transaction(Transaction {
        date,
        flag,
        payee,
        narration,
        tags,
        links,
        meta: txn_meta,
        postings,
    }))
}

fn is_meta_line(tokens: &[(Token, Span)]) -> bool {
    matches!(tokens.first(), Some((Token::Ident(_), _)))
        && matches!(tokens.get(1), Some((Token::Colon, _)))
}

fn parse_meta_lines(
    lines: &[PhysicalLine],
    source: &SourceFile,
    _errors: &mut Vec<ParseError>,
) -> Vec<Metadata> {
    lines
        .iter()
        .filter(|l| is_meta_line(&l.tokens))
        .map(|l| parse_meta_line(l, source))
        .collect()
}

fn parse_meta_line(line: &PhysicalLine, source: &SourceFile) -> Metadata {
    let key = match &line.tokens[0].0 {
        Token::Ident(s) => s.clone(),
        _ => unreachable!("is_meta_line already checked this"),
    };
    let raw_value = if line.tokens.len() > 2 {
        let start = line.tokens[2].1;
        let end = line.tokens.last().unwrap().1;
        let span = start.merge(end);
        source.contents[span.offset..span.offset + span.len]
            .trim()
            .to_string()
    } else {
        String::new()
    };
    Metadata { key, raw_value }
}

fn parse_posting(line: &PhysicalLine, source: &SourceFile) -> Result<Posting, ParseError> {
    let mut cur = Cursor::new(&line.tokens, source);
    let flag = match cur.peek() {
        Some(Token::Star) => {
            cur.bump();
            Some('*')
        }
        Some(Token::Bang) => {
            cur.bump();
            Some('!')
        }
        _ => None,
    };
    let account = cur.expect_account()?;

    let amount = if matches!(cur.peek(), Some(Token::Number(_))) {
        Some(cur.expect_amount()?)
    } else {
        None
    };

    let cost = if matches!(cur.peek(), Some(Token::LBrace)) {
        Some(parse_cost_spec(&mut cur)?)
    } else {
        None
    };

    let price = match cur.peek() {
        Some(Token::At) => {
            cur.bump();
            Some(PriceAnnotation::PerUnit(cur.expect_amount()?))
        }
        Some(Token::AtAt) => {
            cur.bump();
            Some(PriceAnnotation::Total(cur.expect_amount()?))
        }
        _ => None,
    };

    if !cur.at_end() {
        return Err(ParseError::unexpected_token(
            source,
            cur.peek_span().unwrap(),
            "end of posting",
            &cur.peek().unwrap().describe(),
        ));
    }

    Ok(Posting {
        flag,
        account,
        amount,
        cost,
        price,
        meta: Vec::new(),
    })
}

fn parse_cost_spec(cur: &mut Cursor) -> Result<CostSpec, ParseError> {
    cur.bump(); // `{`
    let mut spec = CostSpec::default();
    while !matches!(cur.peek(), Some(Token::RBrace) | None) {
        match cur.peek().cloned().unwrap() {
            Token::Number(_) => spec.per_unit = Some(cur.expect_amount()?),
            Token::Date(d) => {
                cur.bump();
                spec.date = Some(d);
            }
            Token::Str(s) => {
                cur.bump();
                spec.label = Some(s);
            }
            Token::Comma => {
                cur.bump();
            }
            _ => {
                cur.bump();
            }
        }
    }
    if matches!(cur.peek(), Some(Token::RBrace)) {
        cur.bump();
    }
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ob_diagnostics::SourceFile;

    fn parse_ok(text: &str) -> Ledger {
        let source = SourceFile::new("test.beancount", text);
        let (ledger, errors) = parse(&source);
        assert!(errors.is_empty(), "unexpected parse errors: {errors:#?}");
        ledger
    }

    #[test]
    fn parses_open_directives_with_comments() {
        let ledger = parse_ok(
            r#"
2026-05-01 open Assets:Anin:HDFC:Checking
2026-05-01 open Assets:Anin:Kotak:Checking

; Receivables
2026-05-01 open Assets:Anin:Receivable:Moni
"#,
        );
        assert_eq!(ledger.directives.len(), 3);
        match &ledger.directives[0].value {
            Directive::Open { account, .. } => {
                assert_eq!(account.to_string(), "Assets:Anin:HDFC:Checking");
            }
            other => panic!("expected Open, got {other:?}"),
        }
    }

    #[test]
    fn parses_transaction_with_elided_posting_and_multiple_postings_same_account() {
        let ledger = parse_ok(
            r#"
2026-05-01 * "Abad Green Terrace" "Ajzal settlement"
  ; Including room rent
  Assets:Anin:Kotak:Checking          -5105.51 INR
  Assets:Anin:Kotak:Checking          -5964.00 INR
  Expenses:Anin:Housing:Rent          10500.51 INR
  Expenses:Anin:Housing:Utilities       569.00 INR
  Income:Anin:Kotak:Cashback             -2.85 INR
  Assets:Anin:Kotak:Checking             +2.85 INR
"#,
        );
        let Directive::Transaction(txn) = &ledger.directives[0].value else {
            panic!("expected a transaction");
        };
        assert_eq!(txn.payee.as_deref(), Some("Abad Green Terrace"));
        assert_eq!(txn.narration.as_deref(), Some("Ajzal settlement"));
        assert_eq!(txn.postings.len(), 6);
        assert_eq!(
            txn.postings[5]
                .value
                .amount
                .as_ref()
                .unwrap()
                .number
                .to_string(),
            "2.85"
        );
    }

    #[test]
    fn parses_transaction_with_single_elided_posting() {
        let ledger = parse_ok(
            r#"
2026-05-01 * "Ajzal" "ICore - Settlement"
  Assets:Anin:Kotak:Checking          -1710.25 INR
  Expenses:Anin:ICore
"#,
        );
        let Directive::Transaction(txn) = &ledger.directives[0].value else {
            panic!("expected a transaction");
        };
        assert_eq!(txn.postings.len(), 2);
        assert!(txn.postings[1].value.amount.is_none());
    }

    #[test]
    fn parses_balance_option_and_include() {
        let ledger = parse_ok(
            r#"
option "title" "Anin's Personal Finance"
include "beans/2026.beancount"
2026-06-01 balance Assets:Anin:Kotak:Checking 1234.56 INR
"#,
        );
        assert!(matches!(
            ledger.directives[0].value,
            Directive::Option { .. }
        ));
        assert!(matches!(
            ledger.directives[1].value,
            Directive::Include { .. }
        ));
        assert!(matches!(
            ledger.directives[2].value,
            Directive::Balance { .. }
        ));
    }

    #[test]
    fn reports_error_on_stray_indentation() {
        let source = SourceFile::new(
            "test.beancount",
            "  Assets:Foo 1.00 INR\n2026-05-01 open Assets:Foo\n",
        );
        let (_ledger, errors) = parse(&source);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn parses_custom_budget_directive() {
        let ledger = parse_ok(
            r#"2026-05-01 custom "budget" Expenses:Anin:Housing:Rent "monthly" 10500 INR"#,
        );
        let Directive::Custom { name, values, .. } = &ledger.directives[0].value else {
            panic!("expected custom directive");
        };
        assert_eq!(name, "budget");
        assert_eq!(values.len(), 3);
    }
}
