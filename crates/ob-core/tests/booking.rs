use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use rust_decimal_macros::dec;
use ustr::Ustr;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn temp_ledger(name: &str, contents: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ob-core-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn loads_and_books_a_realistic_ledger_end_to_end() {
    let result = ob_core::load(&fixture("anin_sample.beancount"));

    assert!(
        result.parse_errors.is_empty(),
        "unexpected parse errors: {:#?}",
        result
            .parse_errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        result.booking_errors.is_empty(),
        "unexpected booking errors: {:#?}",
        result
            .booking_errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
    );

    assert_eq!(
        result.ledger.meta.title.as_deref(),
        Some("Sample Personal Finance")
    );
    assert_eq!(
        result.ledger.meta.operating_currency.as_deref(),
        Some("INR")
    );

    let checking = ob_core::Account::new("Assets:Sample:Bank:Checking");
    let balance = ob_core::query::balance_at(
        &result.ledger,
        checking,
        NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
    );
    assert_eq!(
        balance.get(&Ustr::from("INR")).copied(),
        Some(dec!(24446.09))
    );
}

#[test]
fn elided_posting_is_interpolated_as_the_exact_residual() {
    let result = ob_core::load(&fixture("anin_sample.beancount"));
    let food = ob_core::Account::new("Expenses:Sample:Food");
    let postings = ob_core::query::postings_for_account(&result.ledger, food);
    assert_eq!(postings.len(), 1);
    let amount = postings[0].1.amount.expect("interpolated");
    assert_eq!(amount.number, dec!(1710.25));
    assert_eq!(amount.currency, Ustr::from("INR"));
}

#[test]
fn unbalanced_transaction_is_reported_not_silently_accepted() {
    let path = temp_ledger(
        "unbalanced.beancount",
        r#"
2026-01-01 open Assets:Test:Checking
2026-01-01 open Expenses:Test:Food

2026-01-02 * "Store" "Snack"
  Assets:Test:Checking   -100.00 INR
  Expenses:Test:Food       90.00 INR
"#,
    );
    let result = ob_core::load(&path);
    assert_eq!(result.booking_errors.len(), 1);
    assert!(
        result.booking_errors[0]
            .message
            .contains("does not balance")
    );
}

#[test]
fn balance_assertion_mismatch_is_reported() {
    let path = temp_ledger(
        "bad_balance.beancount",
        r#"
2026-01-01 open Assets:Test:Checking
2026-01-01 open Income:Test:Salary

2026-01-02 * "Employer" "Payslip"
  Income:Test:Salary       -100.00 INR
  Assets:Test:Checking      100.00 INR

2026-01-03 balance Assets:Test:Checking 999.00 INR
"#,
    );
    let result = ob_core::load(&path);
    assert_eq!(result.booking_errors.len(), 1);
    assert!(
        result.booking_errors[0]
            .message
            .contains("balance assertion failed")
    );
}

#[test]
fn posting_to_unopened_account_is_reported() {
    let path = temp_ledger(
        "unopened.beancount",
        r#"
2026-01-01 open Assets:Test:Checking

2026-01-02 * "Store" "Snack"
  Assets:Test:Checking   -100.00 INR
  Expenses:Test:Food      100.00 INR
"#,
    );
    let result = ob_core::load(&path);
    assert_eq!(result.booking_errors.len(), 1);
    assert!(result.booking_errors[0].message.contains("not open"));
}

#[test]
fn two_elided_postings_in_one_transaction_is_an_error() {
    let path = temp_ledger(
        "double_elided.beancount",
        r#"
2026-01-01 open Assets:Test:Checking
2026-01-01 open Expenses:Test:Food
2026-01-01 open Expenses:Test:Fuel

2026-01-02 * "Store" "Snack"
  Assets:Test:Checking   -100.00 INR
  Expenses:Test:Food
  Expenses:Test:Fuel
"#,
    );
    let result = ob_core::load(&path);
    assert_eq!(result.booking_errors.len(), 1);
    assert!(result.booking_errors[0].message.contains("elided"));
}

#[test]
fn tolerance_scales_with_the_smallest_decimal_place_used() {
    // 100.005 rounds to a 3-decimal posting; half a thousandth (0.0005)
    // must be tolerated, matching beancount's inferred-tolerance rule.
    let path = temp_ledger(
        "tolerance.beancount",
        r#"
2026-01-01 open Assets:Test:Checking
2026-01-01 open Expenses:Test:Food

2026-01-02 * "Store" "Snack"
  Assets:Test:Checking   -100.005 INR
  Expenses:Test:Food      100.0045 INR
"#,
    );
    let result = ob_core::load(&path);
    assert!(
        result.booking_errors.is_empty(),
        "expected the tiny residual to be within tolerance: {:#?}",
        result
            .booking_errors
            .iter()
            .map(|e| e.message.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn reopening_an_open_account_is_an_error() {
    let path = temp_ledger(
        "reopen.beancount",
        r#"
2026-01-01 open Assets:Test:Checking
2026-02-01 open Assets:Test:Checking
"#,
    );
    let result = ob_core::load(&path);
    assert_eq!(result.booking_errors.len(), 1);
    assert!(result.booking_errors[0].message.contains("already open"));
}
