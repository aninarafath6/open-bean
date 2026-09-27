//! The booking pass: walks the date-sorted, lowered directive stream once,
//! maintaining a running per-account inventory. This is where a single
//! elided posting gets interpolated, where transactions are checked to sum
//! to zero, and where `balance` assertions are checked against the running
//! inventory — all within a tolerance derived from the decimal precision
//! actually used, not a hardcoded epsilon.

use std::collections::HashMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;
use ustr::Ustr;

use crate::error::BookingError;
use crate::model::*;

struct AccountState {
    open_date: NaiveDate,
    close_date: Option<NaiveDate>,
    inventory: Inventory,
}

pub struct BookOutput {
    pub ledger: Ledger,
    pub errors: Vec<BookingError>,
}

pub fn book(directives: Vec<Directive>, meta: LedgerMeta) -> BookOutput {
    let mut errors = Vec::new();
    let mut accounts: HashMap<Account, AccountState> = HashMap::new();
    let mut out = Vec::with_capacity(directives.len());

    for directive in directives {
        match directive {
            Directive::Open(open) => {
                book_open(&open, &mut accounts, &mut errors);
                out.push(Directive::Open(open));
            }
            Directive::Close(close) => {
                book_close(&close, &mut accounts, &mut errors);
                out.push(Directive::Close(close));
            }
            Directive::Transaction(mut txn) => {
                if let Err(e) = book_transaction(&mut txn, &mut accounts) {
                    errors.push(e);
                }
                out.push(Directive::Transaction(txn));
            }
            Directive::Balance(bal) => {
                if let Err(e) = check_balance(&bal, &accounts) {
                    errors.push(e);
                }
                out.push(Directive::Balance(bal));
            }
            // Pad's semantics (compute the padding amount needed to satisfy
            // the *next* balance assertion) are explicitly deferred past
            // v1; it passes through with no inventory effect.
            other => out.push(other),
        }
    }

    BookOutput {
        ledger: Ledger {
            directives: out,
            meta,
        },
        errors,
    }
}

fn book_open(
    open: &Open,
    accounts: &mut HashMap<Account, AccountState>,
    errors: &mut Vec<BookingError>,
) {
    match accounts.get(&open.account) {
        Some(state) if state.close_date.is_none() => {
            errors.push(BookingError::account_reopened(
                &open.loc,
                open.account.as_str(),
                state.open_date,
            ));
        }
        _ => {
            accounts.insert(
                open.account,
                AccountState {
                    open_date: open.date,
                    close_date: None,
                    inventory: Inventory::new(),
                },
            );
        }
    }
}

fn book_close(
    close: &Close,
    accounts: &mut HashMap<Account, AccountState>,
    errors: &mut Vec<BookingError>,
) {
    match accounts.get_mut(&close.account) {
        Some(state) if state.close_date.is_none() => state.close_date = Some(close.date),
        Some(_) => errors.push(BookingError::account_already_closed(
            &close.loc,
            close.account.as_str(),
        )),
        None => errors.push(BookingError::account_not_open(
            &close.loc,
            close.account.as_str(),
            close.date,
        )),
    }
}

fn account_open_on(state: &AccountState, date: NaiveDate) -> bool {
    state.open_date <= date && state.close_date.is_none_or(|c| date < c)
}

/// Half a unit of the *least* precise amount involved, matching beancount's
/// inferred-tolerance rule: a posting written with fewer decimal places
/// (e.g. a whole-rupee `100 INR`) could be off by more rounding than one
/// written to the paisa (`100.00 INR`), so the loosest precision present
/// sets the tolerance for the whole check — not a fixed epsilon, which
/// would be too strict for whole-unit entries and too loose for fractional
/// ones.
fn infer_tolerance(amounts: impl Iterator<Item = Decimal>) -> Decimal {
    let min_scale = amounts.map(|a| a.scale()).min().unwrap_or(0);
    Decimal::new(5, min_scale + 1)
}

fn book_transaction(
    txn: &mut Transaction,
    accounts: &mut HashMap<Account, AccountState>,
) -> Result<(), BookingError> {
    for p in &txn.postings {
        match accounts.get(&p.account) {
            Some(state) if account_open_on(state, txn.date) => {}
            Some(state) => {
                return Err(
                    BookingError::account_not_open(&p.loc, p.account.as_str(), txn.date)
                        .with_help(format!("opened {}", state.open_date)),
                );
            }
            None => {
                return Err(BookingError::account_not_open(
                    &p.loc,
                    p.account.as_str(),
                    txn.date,
                ));
            }
        }
    }

    let elided: Vec<usize> = txn
        .postings
        .iter()
        .enumerate()
        .filter(|(_, p)| p.amount.is_none())
        .map(|(i, _)| i)
        .collect();
    if elided.len() > 1 {
        return Err(BookingError::multiple_elided_postings(
            &txn.postings[elided[1]].loc,
        ));
    }
    if let Some(&idx) = elided.first() {
        let mut currency: Option<Ustr> = None;
        let mut sum = Decimal::ZERO;
        for (i, p) in txn.postings.iter().enumerate() {
            if i == idx {
                continue;
            }
            let amt = p.amount.expect("only one posting can be elided");
            match currency {
                None => currency = Some(amt.currency),
                Some(c) if c != amt.currency => {
                    return Err(BookingError::mixed_currency_elision(&txn.postings[idx].loc));
                }
                _ => {}
            }
            sum += amt.number;
        }
        let currency = match currency {
            Some(c) => c,
            None => return Err(BookingError::mixed_currency_elision(&txn.postings[idx].loc)),
        };
        txn.postings[idx].amount = Some(Amount {
            number: -sum,
            currency,
        });
    }

    let mut sums: HashMap<Ustr, Decimal> = HashMap::new();
    for p in &txn.postings {
        let amt = p.amount.expect("elided posting was interpolated above");
        *sums.entry(amt.currency).or_default() += amt.number;
    }
    let tolerance = infer_tolerance(
        txn.postings
            .iter()
            .filter_map(|p| p.amount.map(|a| a.number)),
    );
    for (currency, sum) in &sums {
        if sum.abs() > tolerance {
            return Err(BookingError::does_not_balance(
                &txn.loc,
                currency.as_str(),
                *sum,
            ));
        }
    }

    for p in &txn.postings {
        let amt = p.amount.expect("elided posting was interpolated above");
        let state = accounts.get_mut(&p.account).expect("checked open above");
        *state.inventory.entry(amt.currency).or_default() += amt.number;
    }
    Ok(())
}

fn check_balance(
    bal: &Balance,
    accounts: &HashMap<Account, AccountState>,
) -> Result<(), BookingError> {
    let state = accounts
        .get(&bal.account)
        .ok_or_else(|| BookingError::account_not_open(&bal.loc, bal.account.as_str(), bal.date))?;
    let actual = state
        .inventory
        .get(&bal.amount.currency)
        .copied()
        .unwrap_or(Decimal::ZERO);
    let tolerance = infer_tolerance(std::iter::once(bal.amount.number));
    if (actual - bal.amount.number).abs() > tolerance {
        return Err(BookingError::balance_mismatch(
            &bal.loc,
            bal.account.as_str(),
            bal.amount.number,
            actual,
            bal.amount.currency.as_str(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod proptests {
    use super::*;
    use ob_diagnostics::{SourceFile, Span};
    use proptest::prelude::*;

    fn test_loc() -> Loc {
        Loc {
            source: SourceFile::new("proptest.beancount", ""),
            span: Span::new(0, 0),
        }
    }

    fn open_account(name: &str, accounts: &mut HashMap<Account, AccountState>) -> Account {
        let account = Account::new(name);
        accounts.insert(
            account,
            AccountState {
                open_date: NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
                close_date: None,
                inventory: Inventory::new(),
            },
        );
        account
    }

    fn posting(account: Account, number: Decimal) -> Posting {
        Posting {
            account,
            amount: Some(Amount {
                number,
                currency: Ustr::from("USD"),
            }),
            flag: None,
            meta: Vec::new(),
            loc: test_loc(),
        }
    }

    fn cents(x: i64) -> Decimal {
        Decimal::new(x, 2)
    }

    proptest! {
        #[test]
        fn balanced_transactions_always_book_without_error(
            amounts_cents in prop::collection::vec(-100_000i64..100_000, 2..6)
        ) {
            let mut accounts = HashMap::new();
            let checking = open_account("Assets:Test:Checking", &mut accounts);
            let other = open_account("Expenses:Test:Other", &mut accounts);

            let mut postings: Vec<Posting> = amounts_cents
                .iter()
                .map(|c| posting(checking, cents(*c)))
                .collect();
            let residual = -amounts_cents.iter().sum::<i64>();
            postings.push(posting(other, cents(residual)));

            let mut txn = Transaction {
                date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                flag: '*',
                payee: None,
                narration: None,
                tags: vec![],
                links: vec![],
                meta: vec![],
                postings,
                loc: test_loc(),
            };

            prop_assert!(book_transaction(&mut txn, &mut accounts).is_ok());
            let balance = accounts[&checking].inventory[&Ustr::from("USD")];
            prop_assert_eq!(balance, cents(amounts_cents.iter().sum::<i64>()));
        }

        #[test]
        fn eliding_one_posting_recovers_the_exact_residual(
            amounts_cents in prop::collection::vec(-100_000i64..100_000, 1..6)
        ) {
            let mut accounts = HashMap::new();
            let checking = open_account("Assets:Test:Checking", &mut accounts);
            let other = open_account("Expenses:Test:Other", &mut accounts);

            let mut postings: Vec<Posting> = amounts_cents
                .iter()
                .map(|c| posting(checking, cents(*c)))
                .collect();
            let expected_residual = cents(-amounts_cents.iter().sum::<i64>());
            let mut elided = posting(other, Decimal::ZERO);
            elided.amount = None;
            postings.push(elided);

            let mut txn = Transaction {
                date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                flag: '*',
                payee: None,
                narration: None,
                tags: vec![],
                links: vec![],
                meta: vec![],
                postings,
                loc: test_loc(),
            };

            prop_assert!(book_transaction(&mut txn, &mut accounts).is_ok());
            prop_assert_eq!(txn.postings.last().unwrap().amount.unwrap().number, expected_residual);
        }

        #[test]
        fn unbalanced_transactions_are_always_rejected(
            amounts_cents in prop::collection::vec(-100_000i64..100_000, 1..6),
            off_by_cents in 1i64..10_000
        ) {
            let mut accounts = HashMap::new();
            let checking = open_account("Assets:Test:Checking", &mut accounts);
            let other = open_account("Expenses:Test:Other", &mut accounts);

            let mut postings: Vec<Posting> = amounts_cents
                .iter()
                .map(|c| posting(checking, cents(*c)))
                .collect();
            // Deliberately off by `off_by_cents` instead of the exact residual.
            let residual = -amounts_cents.iter().sum::<i64>() + off_by_cents;
            postings.push(posting(other, cents(residual)));

            let mut txn = Transaction {
                date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                flag: '*',
                payee: None,
                narration: None,
                tags: vec![],
                links: vec![],
                meta: vec![],
                postings,
                loc: test_loc(),
            };

            prop_assert!(book_transaction(&mut txn, &mut accounts).is_err());
        }
    }
}
