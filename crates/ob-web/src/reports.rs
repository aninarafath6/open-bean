//! Ledger -> report data. Deliberately framework-agnostic (no axum/askama
//! here) so it's unit-testable on its own and reusable if a JSON API is
//! added later — it only talks to `ob_core::query`, never touches booking.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use ob_core::{Account, AccountType, Inventory, Ledger};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use ustr::Ustr;

/// Whether a cell reads as good news, bad news, or is simply informational.
/// Spending money or owing a normal liability balance isn't "bad" — only
/// meaningful financial signals (net worth, profit, an overdrawn account)
/// earn color; routine amounts stay neutral so red/green keep their weight.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Positive,
    Negative,
    Neutral,
}

/// One already-formatted table cell: text plus a semantic tone, so
/// templates never reason about signs or account types.
pub struct Cell {
    pub text: String,
    pub tone: Tone,
    /// The raw display value plus the currency affixes, so a hero number can
    /// animate a count-up in JS from the same value the static `text`
    /// renders — kept alongside `text` rather than replacing it, since most
    /// cells (table rows, totals) render `text` directly with no animation.
    pub value: f64,
    pub prefix: String,
    pub suffix: String,
}

impl Cell {
    /// The CSS class for this cell's tone (`""` renders as plain text).
    pub fn class(&self) -> &'static str {
        match self.tone {
            Tone::Positive => "pos",
            Tone::Negative => "neg",
            Tone::Neutral => "",
        }
    }

    pub fn is_negative(&self) -> bool {
        matches!(self.tone, Tone::Negative)
    }
}

pub struct Row {
    /// The leaf name ("Checking") and the path above it ("Anin › Kotak"),
    /// so a row leads with what the account *is* instead of its full path.
    pub title: String,
    pub path: String,
    pub link: Option<String>,
    pub cells: Vec<Cell>,
    /// A WhatsApp-style fallback avatar: a single initial on a colored
    /// circle, colored by the account's root type and lettered by its own
    /// last segment (so "Checking" and "Cash" under Assets both read green
    /// but stay visually distinct from each other).
    pub avatar_letter: String,
    pub avatar_color: &'static str,
}

/// A fixed, brand-distinct color per root account type for row avatars —
/// picked to sit apart from the WhatsApp teal-green accent so a row's
/// avatar never gets mistaken for an interactive/brand element.
fn avatar_color(root: AccountType) -> &'static str {
    match root {
        AccountType::Assets => "#00A884",
        AccountType::Liabilities => "#F0A02B",
        AccountType::Equity => "#8B5CF6",
        AccountType::Income => "#3B82F6",
        AccountType::Expenses => "#EF5A6F",
    }
}

fn title_and_path(account: Account) -> (String, String) {
    let segments: Vec<&str> = account.segments().collect();
    let title = segments.last().copied().unwrap_or_default().to_string();
    let path = if segments.len() > 2 {
        segments[1..segments.len() - 1].join(" › ")
    } else {
        segments.first().copied().unwrap_or_default().to_string()
    };
    (title, path)
}

fn avatar_letter(account: Account) -> String {
    account
        .segments()
        .last()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

pub struct Section {
    pub title: String,
    pub rows: Vec<Row>,
    pub total: Vec<Cell>,
}

/// Grouped number formatting with exactly `dp` decimals ("91,667.00" at 2,
/// "91,667" at 0), the way any polished finance UI presents currency
/// regardless of how it was entered in the source ledger.
pub(crate) fn format_number(amount: Decimal, dp: u32) -> String {
    let mut rounded = amount.round_dp(dp);
    // `round_dp` never *adds* scale (2.5 stays "2.5"), so pin it exactly.
    rounded.rescale(dp);
    let s = rounded.to_string();
    let (negative, digits) = s
        .strip_prefix('-')
        .map_or((false, s.as_str()), |d| (true, d));
    let (int_part, frac_part) = match digits.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (digits, None),
    };
    let grouped: String = int_part
        .as_bytes()
        .iter()
        .rev()
        .enumerate()
        .flat_map(|(i, &b)| {
            if i > 0 && i % 3 == 0 {
                vec![b',', b]
            } else {
                vec![b]
            }
        })
        .rev()
        .map(|b| b as char)
        .collect();
    let sign = if negative
        && (grouped != "0" || frac_part.is_some_and(|f| f.bytes().any(|b| b != b'0')))
    {
        "-"
    } else {
        ""
    };
    match frac_part {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

/// `(prefix, suffix)` to wrap a formatted number with: a symbol in front for
/// currencies that have a well-known one (₹91,667.00), the ISO code after
/// otherwise (91,667.00 XYZ).
pub(crate) fn currency_affixes(code: &str) -> (String, String) {
    let symbol = match code {
        "INR" => Some("₹"),
        "USD" => Some("$"),
        "EUR" => Some("€"),
        "GBP" => Some("£"),
        "JPY" => Some("¥"),
        _ => None,
    };
    match symbol {
        Some(s) => (s.to_string(), String::new()),
        None => (String::new(), format!(" {code}")),
    }
}

/// A formatted amount with its currency, sign placed before the symbol
/// (-₹680.00, not ₹-680.00).
pub(crate) fn money(amount: Decimal, code: &str, dp: u32) -> String {
    let (prefix, suffix) = currency_affixes(code);
    let s = format_number(amount, dp);
    match s.strip_prefix('-') {
        Some(abs) => format!("-{prefix}{abs}{suffix}"),
        None => format!("{prefix}{s}{suffix}"),
    }
}

pub(crate) fn operating_currency(ledger: &Ledger) -> Ustr {
    ledger
        .meta
        .operating_currency
        .as_deref()
        .map(Ustr::from)
        .unwrap_or_else(|| Ustr::from("INR"))
}

/// Beancount's credit-normal accounts (Liabilities/Equity/Income) carry a
/// negative internal balance by convention; `flip` un-negates them so a
/// report reads the way a human expects ("you owe 500", not "-500").
/// `colorize` gates whether the sign becomes red/green at all (see `Tone`).
fn cell(amount: Decimal, currency: &str, flip: bool, colorize: bool) -> Cell {
    let display = if flip { -amount } else { amount };
    let tone = if !colorize {
        Tone::Neutral
    } else if display.is_sign_negative() {
        Tone::Negative
    } else {
        Tone::Positive
    };
    let (prefix, suffix) = currency_affixes(currency);
    Cell {
        text: money(display, currency, 2),
        tone,
        value: display.to_f64().unwrap_or(0.0),
        prefix,
        suffix,
    }
}

/// Only Assets and Income get colorized per-row: an overdrawn asset account
/// or a negative income entry (a refund/clawback) is a genuine signal.
/// Liabilities, Equity, and Expenses are routine facts, not good or bad
/// news, so they stay neutral — color is reserved for where it's meaningful.
fn colorize_for(root: AccountType) -> bool {
    matches!(root, AccountType::Assets | AccountType::Income)
}

fn build_section(
    root: AccountType,
    ledger: &Ledger,
    link_prefix: &str,
    balance_fn: impl Fn(&Ledger, Account) -> Inventory,
) -> Section {
    let flip = !root.normal_balance_is_debit();
    let colorize = colorize_for(root);
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
            cells.push(cell(*amount, currency.as_str(), flip, colorize));
            *totals.entry(*currency).or_default() += *amount;
        }
        let (title, path) = title_and_path(account);
        rows.push(Row {
            title,
            path,
            link: Some(format!("{link_prefix}{account}")),
            cells,
            avatar_letter: avatar_letter(account),
            avatar_color: avatar_color(root),
        });
    }
    let total = totals
        .iter()
        .map(|(c, n)| cell(*n, c.as_str(), flip, colorize))
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

/// Net worth as of `as_of`. The accounting equation (Assets + Liabilities +
/// Equity + Income + Expenses = 0 for the whole ledger) means it falls
/// straight out of a *raw*, unflipped sum of Assets+Liabilities+Equity — no
/// manual sign bookkeeping needed.
pub fn net_worth_now(ledger: &Ledger, as_of: NaiveDate) -> Vec<Cell> {
    let balance_fn = |l: &Ledger, a: Account| ob_core::query::balance_at(l, a, as_of);
    let raw = raw_total(
        ledger,
        &[
            AccountType::Assets,
            AccountType::Liabilities,
            AccountType::Equity,
        ],
        balance_fn,
    );
    raw.iter()
        .map(|(c, n)| cell(*n, c.as_str(), false, true))
        .collect()
}

pub fn balance_sheet(ledger: &Ledger, as_of: NaiveDate) -> BalanceSheet {
    let balance_fn = |l: &Ledger, a: Account| ob_core::query::balance_at(l, a, as_of);
    let assets = build_section(AccountType::Assets, ledger, "/accounts/", balance_fn);
    let liabilities = build_section(AccountType::Liabilities, ledger, "/accounts/", balance_fn);
    let equity = build_section(AccountType::Equity, ledger, "/accounts/", balance_fn);
    let net_worth = net_worth_now(ledger, as_of);

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
        .map(|(c, n)| cell(-*n, c.as_str(), false, true))
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

/// A signed amount meant to read as a *change* (a delta), so a gain shows
/// an explicit `+` the way Apple's Stocks/Wallet apps do — a plain positive
/// number alone doesn't visually register as "up" the way a leading sign does.
pub(crate) fn signed_cell(amount: Decimal, currency: &str) -> Cell {
    let formatted = money(amount, currency, 2);
    let text = if amount.is_sign_negative() {
        formatted
    } else {
        format!("+{formatted}")
    };
    let tone = if amount.is_sign_negative() {
        Tone::Negative
    } else {
        Tone::Positive
    };
    let (prefix, suffix) = currency_affixes(currency);
    Cell {
        text,
        tone,
        value: amount.to_f64().unwrap_or(0.0),
        prefix,
        suffix,
    }
}

pub struct AccountsPage {
    pub as_of: NaiveDate,
    pub net_worth: Vec<Cell>,
    pub sections: Vec<Section>,
}

pub fn accounts_page(ledger: &Ledger, as_of: NaiveDate) -> AccountsPage {
    AccountsPage {
        as_of,
        net_worth: net_worth_now(ledger, as_of),
        sections: accounts_overview(ledger, as_of),
    }
}

/// Change from the series' first point to its last, for a Stocks-app-style
/// "+₹1,234 all-time" line under a hero balance. `None` when there's only
/// one data point (nothing to compare against yet).
pub(crate) fn trend_since_first(series: &[NetWorthPoint], currency: &str) -> Option<Cell> {
    match (series.first(), series.last()) {
        (Some(first), Some(last)) if series.len() > 1 => {
            Some(signed_cell(last.amount - first.amount, currency))
        }
        _ => None,
    }
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
    let root = account.root_type();
    let flip = root.map(|r| !r.normal_balance_is_debit()).unwrap_or(false);
    let colorize = root.map(colorize_for).unwrap_or(false);
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
            amount: cell(amount.number, amount.currency.as_str(), flip, colorize),
            running: cell(running_total, amount.currency.as_str(), flip, colorize),
        });
    }
    let balance = running
        .iter()
        .map(|(c, n)| cell(*n, c.as_str(), flip, colorize))
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

pub(crate) fn end_of_month(date: NaiveDate) -> NaiveDate {
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

pub struct NetWorthPage {
    pub current: Vec<Cell>,
    pub trend: Option<Cell>,
    pub svg: String,
}

pub fn net_worth_page(ledger: &Ledger, as_of: NaiveDate) -> NetWorthPage {
    let current = net_worth_now(ledger, as_of);
    let currency = ledger
        .meta
        .operating_currency
        .as_deref()
        .map(Ustr::from)
        .unwrap_or_else(|| Ustr::from("INR"));
    let series = net_worth_series(ledger, currency);
    let trend = trend_since_first(&series, currency.as_str());
    let svg = render_net_worth_svg(&series, currency.as_str());
    NetWorthPage {
        current,
        trend,
        svg,
    }
}

/// Smooths a polyline into a rounded curve: each interior point becomes a
/// quadratic control point pulling toward the midpoint to its neighbor, the
/// same technique behind most "smooth line" chart modes — no external
/// charting library needed for a curve that doesn't look hand-ruled.
fn smooth_path(points: &[(f64, f64)]) -> String {
    match points.len() {
        0 => String::new(),
        1 => format!("M{:.1},{:.1}", points[0].0, points[0].1),
        2 => format!(
            "M{:.1},{:.1} L{:.1},{:.1}",
            points[0].0, points[0].1, points[1].0, points[1].1
        ),
        _ => {
            let mut d = format!("M{:.1},{:.1} ", points[0].0, points[0].1);
            for i in 1..points.len() - 1 {
                let (cx, cy) = points[i];
                let (nx, ny) = points[i + 1];
                let (midx, midy) = ((cx + nx) / 2.0, (cy + ny) / 2.0);
                d.push_str(&format!("Q{cx:.1},{cy:.1} {midx:.1},{midy:.1} "));
            }
            let (lx, ly) = points[points.len() - 1];
            d.push_str(&format!("L{lx:.1},{ly:.1}"));
            d
        }
    }
}

/// A dependency-free line chart in the style of Apple's Stocks/Wallet apps:
/// a smooth curve, a soft gradient fill beneath it, colored green or red by
/// whether the period trended up or down, that draws itself in on load. No
/// JS charting library, no CDN — just server-rendered SVG, animated in pure
/// CSS (see `.chart-*` in base.html) and disabled under reduced motion.
///
/// `compact` drops axis labels and hoverable points for a sparkline-sized
/// embed (e.g. under a hero stat); the full chart keeps them.
fn render_trend_svg(
    points: &[NetWorthPoint],
    currency: &str,
    width: f64,
    height: f64,
    compact: bool,
) -> String {
    if points.is_empty() {
        return String::new();
    }

    let pad = if compact {
        (2.0, 2.0, 2.0, 2.0)
    } else {
        (64.0, 16.0, 20.0, 32.0)
    };
    let (pad_left, pad_right, pad_top, pad_bottom) = pad;
    let plot_w = width - pad_left - pad_right;
    let plot_h = height - pad_top - pad_bottom;
    let bottom = pad_top + plot_h;

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

    let coords: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, v)| (x_for(i), y_for(*v)))
        .collect();
    let line_path = smooth_path(&coords);
    let (first_x, _) = coords[0];
    let (last_x, _) = coords[coords.len() - 1];
    let area_path = format!("{line_path} L{last_x:.1},{bottom:.1} L{first_x:.1},{bottom:.1} Z");

    // Trending up reads as green, down as red — same convention as Stocks.
    let up = values.last().copied().unwrap_or(0.0) >= values.first().copied().unwrap_or(0.0);
    let color = if up { "var(--pos)" } else { "var(--neg)" };
    let zero_y = y_for(0.0);
    let gradient_id = if compact {
        "netWorthAreaCompact"
    } else {
        "netWorthArea"
    };

    let class = if compact {
        "chart chart-compact"
    } else {
        "chart"
    };
    let mut svg = format!(
        r#"<svg viewBox="0 0 {width} {height}" class="{class}" role="img" aria-label="Net worth over time" preserveAspectRatio="none">"#
    );
    svg.push_str(&format!(
        r#"<defs><linearGradient id="{gradient_id}" x1="0" y1="0" x2="0" y2="1">
             <stop offset="0%" stop-color="{color}" stop-opacity="0.32"/>
             <stop offset="100%" stop-color="{color}" stop-opacity="0"/>
           </linearGradient></defs>"#
    ));
    if !compact && min_v < 0.0 && max_v > 0.0 {
        svg.push_str(&format!(
            r#"<line x1="{pad_left:.1}" y1="{zero_y:.1}" x2="{:.1}" y2="{zero_y:.1}" class="chart-zero" />"#,
            width - pad_right
        ));
    }
    svg.push_str(&format!(
        r#"<path d="{area_path}" class="chart-area" fill="url(#{gradient_id})" stroke="none" />"#
    ));
    svg.push_str(&format!(
        r#"<path d="{line_path}" class="chart-line" fill="none" stroke="{color}" />"#
    ));
    if !compact {
        for (i, &(cx, cy)) in coords.iter().enumerate() {
            svg.push_str(&format!(
                r#"<g class="chart-point-group"><circle cx="{cx:.1}" cy="{cy:.1}" r="10" class="chart-point-hit" fill="transparent" /><circle cx="{cx:.1}" cy="{cy:.1}" r="3" class="chart-point" stroke="{color}"><title>{} {:.2} {currency}</title></circle></g>"#,
                points[i].date, values[i],
            ));
        }
        svg.push_str(&format!(
            r#"<text x="{pad_left:.1}" y="{:.1}" class="chart-label">{}</text>"#,
            height - 8.0,
            points.first().unwrap().date
        ));
        svg.push_str(&format!(
            r#"<text x="{:.1}" y="{:.1}" text-anchor="end" class="chart-label">{}</text>"#,
            width - pad_right,
            height - 8.0,
            points.last().unwrap().date
        ));
        svg.push_str(&format!(
            r#"<text x="4" y="{:.1}" class="chart-label">{max_v:.0}</text>"#,
            pad_top + 4.0
        ));
        svg.push_str(&format!(
            r#"<text x="4" y="{:.1}" class="chart-label">{min_v:.0}</text>"#,
            bottom
        ));
    }
    svg.push_str("</svg>");
    svg
}

pub fn render_net_worth_svg(points: &[NetWorthPoint], currency: &str) -> String {
    if points.is_empty() {
        return "<p class=\"muted\">Not enough data yet.</p>".to_string();
    }
    render_trend_svg(points, currency, 760.0, 280.0, false)
}

/// A compact, label-free trend line for embedding under a hero stat.
pub fn render_net_worth_sparkline(points: &[NetWorthPoint], currency: &str) -> String {
    if points.len() < 2 {
        return String::new();
    }
    render_trend_svg(points, currency, 320.0, 96.0, true)
}

/// Groups an account under the category a human would recognize: the
/// segment right after the root (and an optional personal-namespace
/// segment), so `Expenses:Anin:Housing:Rent` and the direct postings on
/// `Expenses:Anin:Housing` both roll up under "Housing", not fragment into
/// one row per leaf account.
pub(crate) fn category_key_and_label(account: Account) -> (String, String) {
    let segments: Vec<&str> = account.segments().collect();
    match segments.len() {
        0 | 1 => (account.to_string(), account.to_string()),
        2 => (account.to_string(), segments[1].to_string()),
        _ => (segments[0..3].join(":"), segments[2].to_string()),
    }
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
        assert_eq!(sheet.net_worth[0].text, "₹37,223.09");
        assert!(!sheet.net_worth[0].is_negative());
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
            !stmt.net[0].is_negative(),
            "should be profitable: {}",
            stmt.net[0].text
        );
    }

    #[test]
    fn format_money_groups_thousands_and_pads_two_decimals() {
        assert_eq!(format_number(Decimal::new(9166700, 2), 2), "91,667.00");
        assert_eq!(format_number(Decimal::new(50, 0), 2), "50.00");
        assert_eq!(format_number(Decimal::new(-123456, 2), 2), "-1,234.56");
        assert_eq!(format_number(Decimal::new(0, 0), 2), "0.00");
        assert_eq!(format_number(Decimal::new(285, 2), 2), "2.85");
        assert_eq!(format_number(Decimal::new(25, 1), 2), "2.50");
        assert_eq!(format_number(Decimal::new(9166749, 2), 0), "91,667");
        assert_eq!(money(Decimal::new(-68000, 2), "INR", 2), "-₹680.00");
        assert_eq!(money(Decimal::new(500, 0), "XYZ", 0), "500 XYZ");
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
