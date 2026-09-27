//! Ledger -> report data. Deliberately framework-agnostic (no axum/askama
//! here) so it's unit-testable on its own and reusable if a JSON API is
//! added later — it only talks to `ob_core::query`, never touches booking.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use ob_core::{Account, AccountType, Inventory, Ledger};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use ustr::Ustr;

/// One already-formatted table cell: text plus whether to color it as
/// negative, so templates never need to reason about signs.
pub struct Cell {
    pub text: String,
    pub negative: bool,
}

pub struct Row {
    pub label: String,
    pub link: Option<String>,
    pub cells: Vec<Cell>,
}

pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
    pub total: Vec<Cell>,
}

/// Beancount's credit-normal accounts (Liabilities/Equity/Income) carry a
/// negative internal balance by convention; `flip` un-negates them so a
/// report reads the way a human expects ("you owe 500", not "-500").
fn cell(amount: Decimal, currency: &str, flip: bool) -> Cell {
    let display = if flip { -amount } else { amount };
    Cell {
        text: format!("{display} {currency}"),
        negative: display.is_sign_negative(),
    }
}

fn build_section(
    root: AccountType,
    ledger: &Ledger,
    link_prefix: &str,
    balance_fn: impl Fn(&Ledger, Account) -> Inventory,
) -> Section {
    let flip = !root.normal_balance_is_debit();
    let mut rows = Vec::new();
    let mut totals: BTreeMap<Ustr, Decimal> = BTreeMap::new();
    for account in ob_core::query::open_accounts(ledger)
        .into_iter()
        .filter(|a| a.root_type() == Some(root))
    {
        let inventory = balance_fn(ledger, account);
        if inventory.values().all(|v| v.is_zero()) {
            continue;
        }
        let mut cells = Vec::new();
        for (currency, amount) in &inventory {
            cells.push(cell(*amount, currency.as_str(), flip));
            *totals.entry(*currency).or_default() += *amount;
        }
        rows.push(Row {
            label: account.to_string(),
            link: Some(format!("{link_prefix}{account}")),
            cells,
        });
    }
    let total = totals
        .iter()
        .map(|(c, n)| cell(*n, c.as_str(), flip))
        .collect();
    Section {
        title: format!("{root:?}"),
        rows,
        total,
    }
}

fn raw_total(
    ledger: &Ledger,
    roots: &[AccountType],
    balance_fn: impl Fn(&Ledger, Account) -> Inventory,
) -> BTreeMap<Ustr, Decimal> {
    let mut raw = BTreeMap::new();
    for &root in roots {
        for account in ob_core::query::open_accounts(ledger)
            .into_iter()
            .filter(|a| a.root_type() == Some(root))
        {
            for (currency, amount) in balance_fn(ledger, account) {
                *raw.entry(currency).or_default() += amount;
            }
        }
    }
    raw
}

pub struct BalanceSheet {
    pub as_of: NaiveDate,
    pub assets: Section,
    pub liabilities: Section,
    pub equity: Section,
    pub net_worth: Vec<Cell>,
}

pub fn balance_sheet(ledger: &Ledger, as_of: NaiveDate) -> BalanceSheet {
    let balance_fn = |l: &Ledger, a: Account| ob_core::query::balance_at(l, a, as_of);
    let assets = build_section(AccountType::Assets, ledger, "/accounts/", balance_fn);
    let liabilities = build_section(AccountType::Liabilities, ledger, "/accounts/", balance_fn);
    let equity = build_section(AccountType::Equity, ledger, "/accounts/", balance_fn);

    // The accounting equation (Assets + Liabilities + Equity + Income +
    // Expenses = 0 for the whole ledger) means net worth falls straight out
    // of a *raw*, unflipped sum of Assets+Liabilities+Equity — no manual
    // sign bookkeeping needed.
    let raw = raw_total(
        ledger,
        &[
            AccountType::Assets,
            AccountType::Liabilities,
            AccountType::Equity,
        ],
        balance_fn,
    );
    let net_worth = raw
        .iter()
        .map(|(c, n)| cell(*n, c.as_str(), false))
        .collect();

    BalanceSheet {
        as_of,
        assets,
        liabilities,
        equity,
        net_worth,
    }
}

pub struct IncomeStatement {
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub income: Section,
    pub expenses: Section,
    pub net: Vec<Cell>,
}

pub fn income_statement(ledger: &Ledger, start: NaiveDate, end: NaiveDate) -> IncomeStatement {
    let balance_fn = |l: &Ledger, a: Account| ob_core::query::balance_in_range(l, a, start, end);
    let income = build_section(AccountType::Income, ledger, "/accounts/", balance_fn);
    let expenses = build_section(AccountType::Expenses, ledger, "/accounts/", balance_fn);

    let raw = raw_total(
        ledger,
        &[AccountType::Income, AccountType::Expenses],
        balance_fn,
    );
    // Income is credit-normal (negative when earned), Expenses debit-normal
    // (positive when spent); profit = money in minus money out = -(raw sum).
    let net = raw
        .iter()
        .map(|(c, n)| cell(-*n, c.as_str(), false))
        .collect();

    IncomeStatement {
        start,
        end,
        income,
        expenses,
        net,
    }
}

/// One `Section` per root account type, in the conventional balance-sheet
/// order, each showing every open account's current balance. Used by the
/// accounts index page.
pub fn accounts_overview(ledger: &Ledger, as_of: NaiveDate) -> Vec<Section> {
    let balance_fn = |l: &Ledger, a: Account| ob_core::query::balance_at(l, a, as_of);
    [
        AccountType::Assets,
        AccountType::Liabilities,
        AccountType::Equity,
        AccountType::Income,
        AccountType::Expenses,
    ]
    .into_iter()
    .map(|root| build_section(root, ledger, "/accounts/", balance_fn))
    .collect()
}

pub struct PostingRow {
    pub date: NaiveDate,
    pub description: String,
    pub amount: Cell,
    pub running: Cell,
}

pub struct AccountDetail {
    pub account: String,
    pub rows: Vec<PostingRow>,
    pub balance: Vec<Cell>,
}

pub fn account_detail(ledger: &Ledger, account: Account) -> AccountDetail {
    let flip = account
        .root_type()
        .map(|r| !r.normal_balance_is_debit())
        .unwrap_or(false);
    let mut running: BTreeMap<Ustr, Decimal> = BTreeMap::new();
    let mut rows = Vec::new();
    for (txn, posting) in ob_core::query::postings_for_account(ledger, account) {
        let amount = posting
            .amount
            .expect("transactions are booked before querying");
        *running.entry(amount.currency).or_default() += amount.number;
        let running_total = running[&amount.currency];
        let description = match (&txn.payee, &txn.narration) {
            (Some(p), Some(n)) => format!("{p} · {n}"),
            (Some(p), None) => p.clone(),
            (None, Some(n)) => n.clone(),
            (None, None) => String::new(),
        };
        rows.push(PostingRow {
            date: txn.date,
            description,
            amount: cell(amount.number, amount.currency.as_str(), flip),
            running: cell(running_total, amount.currency.as_str(), flip),
        });
    }
    let balance = running
        .iter()
        .map(|(c, n)| cell(*n, c.as_str(), flip))
        .collect();
    AccountDetail {
        account: account.to_string(),
        rows,
        balance,
    }
}

pub struct NetWorthPoint {
    pub date: NaiveDate,
    pub amount: Decimal,
}

fn end_of_month(date: NaiveDate) -> NaiveDate {
    let (year, month) = (date.year(), date.month());
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .unwrap()
        .pred_opt()
        .unwrap()
}

/// A monthly time series of (Assets + Liabilities + Equity) in `currency`,
/// from the ledger's first directive through its last.
pub fn net_worth_series(ledger: &Ledger, currency: Ustr) -> Vec<NetWorthPoint> {
    let Some(first) = ledger.directives.first().map(|d| d.date()) else {
        return Vec::new();
    };
    let last = ledger.directives.last().map(|d| d.date()).unwrap_or(first);

    let accounts: Vec<Account> = ob_core::query::open_accounts(ledger)
        .into_iter()
        .filter(|a| {
            matches!(
                a.root_type(),
                Some(AccountType::Assets | AccountType::Liabilities | AccountType::Equity)
            )
        })
        .collect();

    let total_at = |date: NaiveDate| -> Decimal {
        accounts
            .iter()
            .map(|&a| {
                *ob_core::query::balance_at(ledger, a, date)
                    .get(&currency)
                    .unwrap_or(&Decimal::ZERO)
            })
            .sum()
    };

    let mut points = Vec::new();
    let mut cursor = end_of_month(first).min(last);
    loop {
        points.push(NetWorthPoint {
            date: cursor,
            amount: total_at(cursor),
        });
        if cursor >= last {
            break;
        }
        cursor = end_of_month(cursor + chrono::Duration::days(1)).min(last);
    }
    points
}

/// A dependency-free line chart: no JS charting library, no CDN, just
/// server-rendered SVG embedded straight into the page.
pub fn render_net_worth_svg(points: &[NetWorthPoint], currency: &str) -> String {
    if points.is_empty() {
        return "<p class=\"muted\">Not enough data yet.</p>".to_string();
    }

    let (width, height) = (760.0_f64, 280.0_f64);
    let (pad_left, pad_right, pad_top, pad_bottom) = (70.0_f64, 20.0_f64, 20.0_f64, 40.0_f64);
    let plot_w = width - pad_left - pad_right;
    let plot_h = height - pad_top - pad_bottom;

    let values: Vec<f64> = points
        .iter()
        .map(|p| p.amount.to_f64().unwrap_or(0.0))
        .collect();
    let min_v = values.iter().cloned().fold(0.0_f64, f64::min);
    let max_v = values
        .iter()
        .cloned()
        .fold(0.0_f64, f64::max)
        .max(min_v + 1.0);
    let span = max_v - min_v;

    let x_for = |i: usize| -> f64 {
        if points.len() == 1 {
            pad_left + plot_w / 2.0
        } else {
            pad_left + plot_w * (i as f64) / ((points.len() - 1) as f64)
        }
    };
    let y_for = |v: f64| -> f64 { pad_top + plot_h - (v - min_v) / span * plot_h };

    let mut path = String::new();
    for (i, v) in values.iter().enumerate() {
        path.push_str(&format!(
            "{}{:.1},{:.1} ",
            if i == 0 { "M" } else { "L" },
            x_for(i),
            y_for(*v)
        ));
    }
    let zero_y = y_for(0.0);

    let mut svg = format!(
        r#"<svg viewBox="0 0 {width} {height}" class="chart" role="img" aria-label="Net worth over time">"#
    );
    svg.push_str(&format!(
        r#"<line x1="{pad_left:.1}" y1="{zero_y:.1}" x2="{:.1}" y2="{zero_y:.1}" class="chart-zero" />"#,
        width - pad_right
    ));
    svg.push_str(&format!(
        r#"<path d="{}" class="chart-line" fill="none" />"#,
        path.trim_end()
    ));
    for (i, v) in values.iter().enumerate() {
        svg.push_str(&format!(
            r#"<circle cx="{:.1}" cy="{:.1}" r="3" class="chart-point"><title>{} {v:.2} {currency}</title></circle>"#,
            x_for(i),
            y_for(*v),
            points[i].date,
        ));
    }
    svg.push_str(&format!(
        r#"<text x="{pad_left:.1}" y="{:.1}" class="chart-label">{}</text>"#,
        height - 10.0,
        points.first().unwrap().date
    ));
    svg.push_str(&format!(
        r#"<text x="{:.1}" y="{:.1}" text-anchor="end" class="chart-label">{}</text>"#,
        width - pad_right,
        height - 10.0,
        points.last().unwrap().date
    ));
    svg.push_str(&format!(
        r#"<text x="5" y="{:.1}" class="chart-label">{max_v:.0} {currency}</text>"#,
        pad_top + 5.0
    ));
    svg.push_str(&format!(
        r#"<text x="5" y="{:.1}" class="chart-label">{min_v:.0} {currency}</text>"#,
        height - pad_bottom
    ));
    svg.push_str("</svg>");
    svg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn load_fixture() -> Ledger {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../ob-core/tests/fixtures/anin_sample.beancount");
        ob_core::load(&path).ledger
    }

    #[test]
    fn balance_sheet_nets_out_via_the_accounting_equation() {
        let ledger = load_fixture();
        let sheet = balance_sheet(&ledger, NaiveDate::from_ymd_opt(2026, 1, 10).unwrap());
        assert_eq!(sheet.net_worth.len(), 1);
        // Assets:Checking = 24446.09; the fixture's only Liabilities posting
        // is a paydown with no matching prior charge, so it nets to a raw
        // +12777.00 rather than the usual negative credit-normal balance.
        // Assets(24446.09) + Liabilities(12777.00) + Equity(0) = 37223.09.
        assert_eq!(sheet.net_worth[0].text, "37223.09 INR");
        assert!(!sheet.net_worth[0].negative);
    }

    #[test]
    fn income_statement_shows_positive_profit_when_earning_more_than_spending() {
        let ledger = load_fixture();
        let stmt = income_statement(
            &ledger,
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap(),
        );
        assert_eq!(stmt.net.len(), 1);
        assert!(
            !stmt.net[0].negative,
            "should be profitable: {}",
            stmt.net[0].text
        );
    }

    #[test]
    fn net_worth_series_ends_at_the_last_directive_date() {
        let ledger = load_fixture();
        let series = net_worth_series(&ledger, Ustr::from("INR"));
        assert_eq!(
            series.last().unwrap().date,
            NaiveDate::from_ymd_opt(2026, 1, 10).unwrap()
        );
    }
}
