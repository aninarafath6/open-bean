//! A minimal read API over a booked [`Ledger`] that reports (balance sheet,
//! income statement, net worth) build on, so no report needs its own
//! booking logic — it just asks for balances or postings.

use chrono::NaiveDate;

use crate::model::{Account, Directive, Inventory, Ledger, Posting, Transaction};

/// Every account ever `open`ed, in the order they were opened.
pub fn open_accounts(ledger: &Ledger) -> Vec<Account> {
    ledger
        .directives
        .iter()
        .filter_map(|d| match d {
            Directive::Open(o) => Some(o.account),
            _ => None,
        })
        .collect()
}

/// The account's running balance as of (and including) `as_of`. Relies on
/// `ledger.directives` being date-sorted, which `loader::load` guarantees.
pub fn balance_at(ledger: &Ledger, account: Account, as_of: NaiveDate) -> Inventory {
    let mut inventory = Inventory::new();
    for directive in &ledger.directives {
        if directive.date() > as_of {
            break;
        }
        if let Directive::Transaction(txn) = directive {
            for posting in &txn.postings {
                if posting.account == account {
                    let amount = posting
                        .amount
                        .expect("transactions are booked before querying");
                    *inventory.entry(amount.currency).or_default() += amount.number;
                }
            }
        }
    }
    inventory
}

/// Every posting to `account`, alongside the transaction it belongs to, in
/// date order.
pub fn postings_for_account(ledger: &Ledger, account: Account) -> Vec<(&Transaction, &Posting)> {
    ledger
        .directives
        .iter()
        .filter_map(|d| match d {
            Directive::Transaction(txn) => Some(txn),
            _ => None,
        })
        .flat_map(|txn| txn.postings.iter().map(move |p| (txn, p)))
        .filter(|(_, p)| p.account == account)
        .collect()
}

/// All transactions with a date in `[start, end]`, for period reports
/// (income statement, budget-vs-actual).
pub fn transactions_in_range(ledger: &Ledger, start: NaiveDate, end: NaiveDate) -> Vec<&Transaction> {
    ledger
        .directives
        .iter()
        .filter(|d| d.date() >= start && d.date() <= end)
        .filter_map(|d| match d {
            Directive::Transaction(txn) => Some(txn),
            _ => None,
        })
        .collect()
}
