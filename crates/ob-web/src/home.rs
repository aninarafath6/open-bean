//! The home screen: one month at a time, answering "how much did I spend,
//! on what, and where did it go" — nothing else. Framework-agnostic like
//! `reports`; templates only lay it out.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use ob_core::{AccountType, Directive, Ledger, Transaction};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use ustr::Ustr;

use crate::reports::{
    Cell, category_key_and_label, end_of_month, money, money_html, natural_dp, net_worth_now,
    net_worth_series, operating_currency, render_net_worth_sparkline, trend_since_first,
};

type MonthKey = (i32, u32);

/// One day's spending in the month strip.
pub struct Day {
    /// Bar height relative to the month's biggest day, 0–100.
    pub height: f64,
    pub title: String,
    pub any: bool,
}

pub struct Category {
    pub label: String,
    pub amount: String,
    pub pct: String,
    /// Bar width relative to the biggest category, 0–100.
    pub width: f64,
}

/// One transaction card: who, when, how much, and an uppercase category
/// pill ("FOOD", "INCOME", "TRANSFER").
pub struct Activity {
    pub title: String,
    pub when: String,
    pub amount_html: String,
    pub category: String,
    pub class: &'static str,
}

pub struct Home {
    /// "GOOD EVENING, ANIN" and "IT'S 27TH OF SEPTEMBER, A SUNDAY".
    pub greeting: String,
    pub dateline: String,
    pub month_label: String,
    pub prev_href: Option<String>,
    pub next_href: Option<String>,
    pub spent_html: String,
    pub income_text: String,
    pub saved_text: String,
    /// Whether `saved_text` is good news (kept money) or bad (overspent).
    pub saved_up: bool,
    pub axis_start: String,
    pub axis_end: String,
    /// Share of income spent, 0–100 (100 when there's no income).
    pub progress: f64,
    pub days: Vec<Day>,
    pub categories: Vec<Category>,
    pub activity: Vec<Activity>,
    pub txn_count: usize,
    pub net_worth_html: Option<String>,
    pub net_trend: Option<Cell>,
    pub sparkline: String,
}

fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=16 => "Good afternoon",
        _ => "Good evening",
    }
}

fn ordinal(n: u32) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// "Anin's Personal Finance" -> "Anin"; `None` when the title isn't a
/// possessive, so the greeting just omits the name.
fn owner_name(title: Option<&str>) -> Option<String> {
    title
        .and_then(|t| t.split_once("'s").or_else(|| t.split_once("’s")))
        .map(|(name, _)| name.trim().to_string())
        .filter(|n| !n.is_empty() && n.len() <= 24)
}

fn transactions(ledger: &Ledger) -> impl Iterator<Item = &Transaction> {
    ledger.directives.iter().filter_map(|d| match d {
        Directive::Transaction(t) => Some(t),
        _ => None,
    })
}

fn month_start((y, m): MonthKey) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, 1).expect("valid month")
}

fn month_href((y, m): MonthKey) -> String {
    format!("/?month={y}-{m:02}")
}

fn parse_month(s: &str) -> Option<MonthKey> {
    let (y, m) = s.split_once('-')?;
    let (y, m) = (y.parse().ok()?, m.parse().ok()?);
    (1..=12).contains(&m).then_some((y, m))
}

/// Income earned and money spent in one transaction, in `currency`, from
/// the user's perspective (both as positive numbers).
fn flows(txn: &Transaction, currency: Ustr) -> (Decimal, Decimal) {
    let mut income = Decimal::ZERO;
    let mut spent = Decimal::ZERO;
    for p in &txn.postings {
        let Some(a) = p.amount.filter(|a| a.currency == currency) else {
            continue;
        };
        match p.account.root_type() {
            Some(AccountType::Income) => income -= a.number,
            Some(AccountType::Expenses) => spent += a.number,
            _ => {}
        }
    }
    (income, spent)
}

fn title(t: &Transaction) -> String {
    match (&t.payee, &t.narration) {
        (Some(p), Some(n)) => format!("{p} · {n}"),
        (Some(p), None) => p.clone(),
        (None, Some(n)) => n.clone(),
        (None, None) => "Transaction".to_string(),
    }
}

pub fn home(ledger: &Ledger, month: Option<&str>, now: NaiveDateTime) -> Home {
    let today = now.date();
    let currency = operating_currency(ledger);
    let code = currency.as_str();

    let mut flow_by_month: BTreeMap<MonthKey, (Decimal, Decimal)> = BTreeMap::new();
    for t in transactions(ledger) {
        let (inc, sp) = flows(t, currency);
        let entry = flow_by_month
            .entry((t.date.year(), t.date.month()))
            .or_default();
        entry.0 += inc;
        entry.1 += sp;
    }

    // Every calendar month from the first transaction to the last, gaps
    // included, so stepping month to month never silently skips one.
    let keys: Vec<MonthKey> = match (
        flow_by_month.keys().next(),
        flow_by_month.keys().next_back(),
    ) {
        (Some(&first), Some(&last)) => {
            let mut keys = vec![first];
            while *keys.last().unwrap() < last {
                let (y, m) = *keys.last().unwrap();
                keys.push(if m == 12 { (y + 1, 1) } else { (y, m + 1) });
            }
            keys
        }
        _ => vec![(today.year(), today.month())],
    };

    // Default to the latest month with real money movement — a month holding
    // only a transfer between own accounts would open on an empty ₹0.
    let latest = flow_by_month
        .iter()
        .rev()
        .find(|(_, (inc, sp))| !inc.is_zero() || !sp.is_zero())
        .map(|(k, _)| *k)
        .unwrap_or(*keys.last().unwrap());
    let selected = month
        .and_then(parse_month)
        .filter(|k| keys.contains(k))
        .unwrap_or(latest);
    let idx = keys.iter().position(|k| *k == selected).unwrap_or(0);
    let (start, end) = (month_start(selected), end_of_month(month_start(selected)));

    let mut income = Decimal::ZERO;
    let mut spent = Decimal::ZERO;
    let mut txn_count = 0;
    let mut by_day = vec![Decimal::ZERO; end.day() as usize];
    let mut by_category: BTreeMap<String, (String, Decimal)> = BTreeMap::new();
    let mut activity = Vec::new();
    for t in transactions(ledger).filter(|t| t.date >= start && t.date <= end) {
        txn_count += 1;
        let (inc, sp) = flows(t, currency);
        income += inc;
        spent += sp;
        by_day[t.date.day0() as usize] += sp;

        let mut biggest: Option<(String, Decimal)> = None;
        for p in &t.postings {
            let Some(a) = p.amount.filter(|a| a.currency == currency) else {
                continue;
            };
            if p.account.root_type() != Some(AccountType::Expenses) {
                continue;
            }
            let (key, label) = category_key_and_label(p.account);
            by_category
                .entry(key)
                .or_insert((label.clone(), Decimal::ZERO))
                .1 += a.number;
            if biggest.as_ref().is_none_or(|(_, n)| a.number > *n) {
                biggest = Some((label, a.number));
            }
        }

        let when = t.date.format("%-d %B").to_string();
        let item = if sp > Decimal::ZERO {
            Activity {
                title: title(t),
                when,
                amount_html: money_html(-sp, code, natural_dp(sp)),
                category: biggest
                    .map(|(l, _)| l)
                    .unwrap_or_else(|| "Spending".into())
                    .to_uppercase(),
                class: "",
            }
        } else if inc > Decimal::ZERO {
            Activity {
                title: title(t),
                when,
                amount_html: format!("+{}", money_html(inc, code, natural_dp(inc))),
                category: "INCOME".into(),
                class: "pos",
            }
        } else {
            // A transfer between the user's own accounts: show how much moved.
            let moved: Decimal = t
                .postings
                .iter()
                .filter_map(|p| {
                    p.amount
                        .filter(|a| a.currency == currency && a.number > Decimal::ZERO)
                })
                .map(|a| a.number)
                .sum();
            Activity {
                title: title(t),
                when,
                amount_html: money_html(moved, code, natural_dp(moved)),
                category: "TRANSFER".into(),
                class: "muted",
            }
        };
        activity.push(item);
    }
    let activity: Vec<Activity> = activity.into_iter().rev().take(12).collect();

    let max_day = by_day
        .iter()
        .copied()
        .max()
        .unwrap_or_default()
        .max(Decimal::ONE);
    let days = by_day
        .iter()
        .enumerate()
        .map(|(i, amt)| {
            let date = start + chrono::Duration::days(i as i64);
            Day {
                height: if amt.is_zero() {
                    0.0
                } else {
                    ((*amt / max_day).to_f64().unwrap_or(0.0) * 100.0).max(6.0)
                },
                title: format!("{} · {}", date.format("%b %-d"), money(*amt, code, 0)),
                any: !amt.is_zero(),
            }
        })
        .collect();

    let mut sorted: Vec<(String, Decimal)> = by_category
        .into_values()
        .filter(|(_, n)| *n > Decimal::ZERO)
        .collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    let largest = sorted.first().map(|(_, n)| *n).unwrap_or(Decimal::ONE);
    // A short list reads at a glance; the tail rolls into "Other".
    const MAX_CATEGORIES: usize = 7;
    if sorted.len() > MAX_CATEGORIES {
        let rest: Decimal = sorted
            .split_off(MAX_CATEGORIES - 1)
            .into_iter()
            .map(|(_, n)| n)
            .sum();
        sorted.push(("Other".to_string(), rest));
    }
    let categories = sorted
        .iter()
        .map(|(label, amount)| {
            let share = if spent.is_zero() {
                0.0
            } else {
                (*amount / spent).to_f64().unwrap_or(0.0)
            };
            Category {
                label: label.clone(),
                amount: money(*amount, code, 0),
                pct: if share > 0.0 && share < 0.005 {
                    "<1%".into()
                } else {
                    format!("{:.0}%", share * 100.0)
                },
                width: ((*amount / largest).to_f64().unwrap_or(0.0) * 100.0).clamp(1.0, 100.0),
            }
        })
        .collect();

    let saved_up = income > Decimal::ZERO && spent <= income;
    let (progress, saved_text) = if income > Decimal::ZERO {
        let saved = ((income - spent) / income).to_f64().unwrap_or(0.0);
        let p = (spent / income).to_f64().unwrap_or(0.0).clamp(0.0, 1.0) * 100.0;
        let text = if saved >= 0.0 {
            format!("{:.0}% saved", saved * 100.0)
        } else {
            format!("{:.0}% over income", -saved * 100.0)
        };
        (p, text)
    } else {
        (
            if spent > Decimal::ZERO { 100.0 } else { 0.0 },
            "No income this month".into(),
        )
    };

    // Net worth at the end of the month shown (not projected past the ledger).
    let nw_date = ledger
        .directives
        .last()
        .map(|d| d.date().min(end))
        .unwrap_or(end);
    let net_worth_html = net_worth_now(ledger, nw_date)
        .into_iter()
        .find(|c| c.currency == code)
        .map(|c| c.html);
    let series = net_worth_series(ledger, currency);
    let net_trend = trend_since_first(&series, code);
    let sparkline = render_net_worth_sparkline(&series, code);

    let greeting = match owner_name(ledger.meta.title.as_deref()) {
        Some(name) => format!("{}, {name}", greeting(now.hour())),
        None => greeting(now.hour()).to_string(),
    };
    let dateline = format!(
        "It's {} of {}, a {}",
        ordinal(today.day()),
        today.format("%B"),
        today.format("%A")
    );

    Home {
        greeting,
        dateline,
        month_label: start.format("%B %Y").to_string(),
        prev_href: idx.checked_sub(1).map(|i| month_href(keys[i])),
        next_href: keys.get(idx + 1).map(|k| month_href(*k)),
        spent_html: money_html(spent, code, 0),
        income_text: money(income, code, 0),
        saved_text,
        saved_up,
        axis_start: start.format("1 %b").to_string(),
        axis_end: end.format("%-d %b").to_string(),
        progress,
        days,
        categories,
        activity,
        txn_count,
        net_worth_html,
        net_trend,
        sparkline,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture() -> Ledger {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../ob-core/tests/fixtures/anin_sample.beancount");
        ob_core::load(&path).ledger
    }

    fn today() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 1, 20)
            .unwrap()
            .and_hms_opt(19, 30, 0)
            .unwrap()
    }

    #[test]
    fn home_summarizes_the_latest_month() {
        let h = home(&fixture(), None, today());
        assert_eq!(h.month_label, "January 2026");
        // Salary 50,000 + the 2.85 cashback folded into the rent settlement.
        assert_eq!(h.income_text, "₹50,003");
        // Rent 10,500.51 + Utilities 569.00 + Food 1,710.25
        assert!(h.spent_html.ends_with("12,780"), "{}", h.spent_html);
        assert_eq!(h.saved_text, "74% saved");
        assert_eq!(h.categories[0].label, "Housing");
        assert_eq!(h.txn_count, 4);
        assert_eq!(h.days.len(), 31);
        assert_eq!(
            (h.prev_href.as_deref(), h.next_href.as_deref()),
            (None, None)
        );
    }

    #[test]
    fn unknown_month_falls_back_to_latest() {
        assert_eq!(
            home(&fixture(), Some("1999-01"), today()).month_label,
            "January 2026"
        );
    }

    #[test]
    fn defaults_past_transfer_only_months_and_steps_through_gaps() {
        let dir = std::env::temp_dir().join(format!("ob-home-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gaps.beancount");
        std::fs::write(
            &path,
            r#"
2026-01-01 open Assets:Bank
2026-01-01 open Assets:Cash
2026-01-01 open Income:Salary
2026-01-01 open Expenses:Food
2026-01-05 * "Employer" "Pay"
  Income:Salary  -1000 INR
  Assets:Bank     1000 INR
2026-01-06 * "Cafe" "Lunch"
  Expenses:Food   100 INR
  Assets:Bank    -100 INR
2026-03-02 * "ATM" "Withdrawal"
  Assets:Cash     200 INR
  Assets:Bank    -200 INR
"#,
        )
        .unwrap();
        let ledger = ob_core::load(&path).ledger;
        let h = home(&ledger, None, today());
        assert_eq!(h.month_label, "January 2026", "March only has a transfer");
        assert_eq!(
            h.next_href.as_deref(),
            Some("/?month=2026-02"),
            "February is a gap, not skipped"
        );
        let feb = home(&ledger, Some("2026-02"), today());
        assert!(feb.spent_html.ends_with(">0"), "{}", feb.spent_html);
        assert_eq!(feb.saved_text, "No income this month");
    }

    #[test]
    fn transfers_show_as_neutral_moves_newest_first() {
        let h = home(&fixture(), None, today());
        let card = &h.activity[0];
        assert_eq!(card.when, "5 January", "newest first");
        assert_eq!((card.category.as_str(), card.class), ("TRANSFER", "muted"));
        assert!(
            card.amount_html.ends_with("12,777"),
            "whole amounts drop the .00: {}",
            card.amount_html
        );
    }

    #[test]
    fn greeting_and_dateline_read_like_a_sentence() {
        let h = home(&fixture(), None, today());
        assert_eq!(
            h.greeting, "Good evening",
            "fixture title has no possessive name"
        );
        assert_eq!(h.dateline, "It's 20th of January, a Tuesday");
        assert_eq!(
            owner_name(Some("Anin's Personal Finance")).as_deref(),
            Some("Anin")
        );
        assert_eq!(
            (ordinal(1), ordinal(2), ordinal(3), ordinal(11), ordinal(22)),
            (
                "1st".into(),
                "2nd".into(),
                "3rd".into(),
                "11th".into(),
                "22nd".into()
            )
        );
    }
}
