//! The home screen: a month-at-a-glance view built around one question —
//! "how much of this month's income is left?" — plus the supporting
//! context (net worth, savings rate, where the money went, what happened
//! recently). Framework-agnostic like `reports`; templates only format.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate, Timelike};
use ob_core::{AccountType, Directive, Ledger, Transaction};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use ustr::Ustr;

use crate::reports::{
    Cell, category_key_and_label, currency_affixes, end_of_month, money, net_worth_now,
    net_worth_series, operating_currency, render_net_worth_sparkline, trend_since_first,
};

type MonthKey = (i32, u32);

pub struct MonthPill {
    pub label: String,
    pub href: String,
    pub active: bool,
}

pub struct Tile {
    pub emoji: &'static str,
    pub label: String,
    pub amount: String,
    pub pct: String,
    /// Bar width relative to the largest category, 0–100.
    pub width: f64,
}

pub struct Bar {
    pub label: String,
    pub amount: String,
    /// Bar height relative to the biggest month shown, 0–100.
    pub height: f64,
    pub active: bool,
    pub href: String,
}

pub struct Activity {
    pub emoji: &'static str,
    pub title: String,
    pub subtitle: String,
    pub amount: String,
    pub class: &'static str,
}

pub struct Home {
    pub greeting: &'static str,
    pub name: String,
    pub initial: String,
    pub month_label: String,
    pub months: Vec<MonthPill>,

    pub gauge_svg: String,
    pub gauge_value: f64,
    pub gauge_text: String,
    pub gauge_caption: String,
    pub prefix: String,
    pub suffix: String,
    pub income_text: String,
    pub spent_text: String,

    pub net_worth: Option<Cell>,
    pub net_trend: Option<Cell>,
    pub sparkline: String,

    pub savings_pct: String,
    /// Filled segments out of `SEGMENTS`, for the segmented progress bar.
    pub savings_filled: usize,
    pub savings_caption: String,
    pub txn_count: usize,
    pub top_category: Option<Tile>,

    pub bars: Vec<Bar>,
    pub bars_total: String,
    pub tiles: Vec<Tile>,
    pub recent: Vec<Activity>,
}

pub const SEGMENTS: usize = 8;

impl Home {
    /// `0..SEGMENTS` paired with whether each segment is filled, so the
    /// template can render the segmented bar without arithmetic.
    pub fn segments(&self) -> Vec<bool> {
        (0..SEGMENTS).map(|i| i < self.savings_filled).collect()
    }
}

fn greeting(hour: u32) -> &'static str {
    match hour {
        5..=11 => "Good morning",
        12..=16 => "Good afternoon",
        _ => "Good evening",
    }
}

/// "Anin's Personal Finance" -> "Anin". Falls back to a neutral "there" so
/// the greeting always reads naturally.
fn owner_name(title: Option<&str>) -> String {
    title
        .and_then(|t| t.split_once("'s").or_else(|| t.split_once("’s")))
        .map(|(name, _)| name.trim().to_string())
        .filter(|n| !n.is_empty() && n.len() <= 24)
        .unwrap_or_else(|| "there".to_string())
}

/// A friendly glyph per spending category, keyed on the words people
/// actually use for their accounts. Unknown categories get a neutral one.
pub fn category_emoji(label: &str) -> &'static str {
    let l = label.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| l.contains(w));
    if has(&["rent", "housing", "house"]) {
        "🏠"
    } else if has(&["home", "family", "remit"]) {
        "🏡"
    } else if has(&[
        "food",
        "breakfast",
        "lunch",
        "dinner",
        "grocer",
        "restaurant",
        "snack",
    ]) {
        "🍽️"
    } else if has(&["fuel", "petrol", "diesel", "gas"]) {
        "⛽"
    } else if has(&[
        "travel",
        "trip",
        "flight",
        "train",
        "taxi",
        "cab",
        "transport",
    ]) {
        "✈️"
    } else if has(&["gift"]) {
        "🎁"
    } else if has(&["appear", "cloth", "fashion", "salon", "hair", "beauty"]) {
        "👕"
    } else if has(&["tax", "lwf", "labor", "labour", "fee"]) {
        "🧾"
    } else if has(&[
        "util", "electric", "water", "internet", "phone", "mobile", "solar",
    ]) {
        "💡"
    } else if has(&["health", "medic", "pharma", "doctor", "fitness", "gym"]) {
        "💊"
    } else if has(&[
        "subscri",
        "netflix",
        "hotstar",
        "spotify",
        "entertain",
        "movie",
    ]) {
        "🎬"
    } else if has(&["invest", "sip", "stock", "mutual"]) {
        "📈"
    } else if has(&["emi", "loan", "interest"]) {
        "🏦"
    } else if has(&["educat", "book", "course"]) {
        "📚"
    } else {
        "💸"
    }
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

pub fn home(ledger: &Ledger, month: Option<&str>, now: chrono::DateTime<chrono::Local>) -> Home {
    let currency = operating_currency(ledger);
    let code = currency.as_str();
    let (prefix, suffix) = currency_affixes(code);

    // (income, spent) per month that has any transaction.
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
    // included, so the timeline and bar chart show quiet months as quiet
    // rather than silently skipping them.
    let today = now.date_naive();
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
    let spent_by_month: BTreeMap<MonthKey, Decimal> = keys
        .iter()
        .map(|k| (*k, flow_by_month.get(k).map(|f| f.1).unwrap_or_default()))
        .collect();

    // Default to the latest month with real money movement — a month holding
    // only a transfer between own accounts would open on an empty ₹0 gauge.
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
    let (start, end) = (month_start(selected), end_of_month(month_start(selected)));

    // Pills: up to six months around the selection, oldest first like a timeline.
    let window_end = keys
        .iter()
        .position(|k| *k == selected)
        .map(|i| (i + 3).min(keys.len()))
        .unwrap_or(keys.len());
    let window_start = window_end.saturating_sub(6);
    let window = &keys[window_start..window_end];
    let months = window
        .iter()
        .map(|&k| MonthPill {
            label: month_start(k).format("%b").to_string(),
            href: format!("/?month={}-{:02}", k.0, k.1),
            active: k == selected,
        })
        .collect();

    // This month's flows, categories, and activity.
    let mut income = Decimal::ZERO;
    let mut spent = Decimal::ZERO;
    let mut txn_count = 0;
    let mut by_category: BTreeMap<String, (String, Decimal)> = BTreeMap::new();
    let mut recent = Vec::new();
    for t in transactions(ledger).filter(|t| t.date >= start && t.date <= end) {
        txn_count += 1;
        let (inc, sp) = flows(t, currency);
        income += inc;
        spent += sp;

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
        recent.push((t, inc, sp, biggest.map(|(l, _)| l)));
    }

    let title = |t: &Transaction| match (&t.payee, &t.narration) {
        (Some(p), Some(n)) => format!("{p} · {n}"),
        (Some(p), None) => p.clone(),
        (None, Some(n)) => n.clone(),
        (None, None) => "Transaction".to_string(),
    };
    let recent: Vec<Activity> = recent
        .into_iter()
        .rev()
        .take(6)
        .map(|(t, inc, sp, category)| {
            let date = t.date.format("%b %-d").to_string();
            if sp > Decimal::ZERO {
                let category = category.unwrap_or_else(|| "Spending".to_string());
                Activity {
                    emoji: category_emoji(&category),
                    title: title(t),
                    subtitle: format!("{date} · {category}"),
                    amount: money(-sp, code, 2),
                    class: "",
                }
            } else if inc > Decimal::ZERO {
                Activity {
                    emoji: "💰",
                    title: title(t),
                    subtitle: format!("{date} · Income"),
                    amount: format!("+{}", money(inc, code, 2)),
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
                    emoji: "🔁",
                    title: title(t),
                    subtitle: format!("{date} · Transfer"),
                    amount: money(moved, code, 2),
                    class: "muted",
                }
            }
        })
        .collect();

    let mut categories: Vec<(String, Decimal)> = by_category
        .into_values()
        .filter(|(_, n)| *n > Decimal::ZERO)
        .collect();
    categories.sort_by(|a, b| b.1.cmp(&a.1));
    let largest = categories.first().map(|(_, n)| *n).unwrap_or(Decimal::ONE);
    let tile = |(label, amount): &(String, Decimal)| {
        let share = if spent.is_zero() {
            0.0
        } else {
            (*amount / spent).to_f64().unwrap_or(0.0)
        };
        Tile {
            emoji: category_emoji(label),
            label: label.clone(),
            amount: money(*amount, code, 0),
            pct: if share > 0.0 && share < 0.005 {
                "<1%".to_string()
            } else {
                format!("{:.0}%", share * 100.0)
            },
            width: ((*amount / largest).to_f64().unwrap_or(0.0) * 100.0).clamp(4.0, 100.0),
        }
    };
    let tiles: Vec<Tile> = categories.iter().take(9).map(tile).collect();
    let top_category = categories.first().map(tile);

    // The gauge: how much of this month's income has been spent.
    let progress = if income > Decimal::ZERO {
        (spent / income).to_f64().unwrap_or(0.0).clamp(0.0, 1.0)
    } else if spent > Decimal::ZERO {
        1.0
    } else {
        0.0
    };
    let (gauge_amount, gauge_caption) = if income > Decimal::ZERO {
        let left = income - spent;
        let caption = if left.is_sign_negative() {
            format!("over your {} income", money(income, code, 0))
        } else {
            format!("left of {} income", money(income, code, 0))
        };
        (left, caption)
    } else {
        (spent, "spent this month".to_string())
    };

    let savings = if income > Decimal::ZERO {
        ((income - spent) / income).to_f64().unwrap_or(0.0)
    } else {
        0.0
    };
    let savings_filled =
        ((savings.clamp(0.0, 1.0) * SEGMENTS as f64).round() as usize).min(SEGMENTS);
    let savings_caption = if income > Decimal::ZERO {
        format!(
            "Kept {} of {}",
            money((income - spent).max(Decimal::ZERO), code, 0),
            money(income, code, 0)
        )
    } else {
        "No income recorded this month".to_string()
    };

    // Spending per month for the bar chart, over the same window as the pills.
    let max_month = window
        .iter()
        .filter_map(|k| spent_by_month.get(k))
        .copied()
        .max()
        .unwrap_or(Decimal::ONE)
        .max(Decimal::ONE);
    let bars = window
        .iter()
        .map(|&k| {
            let amt = spent_by_month.get(&k).copied().unwrap_or_default();
            Bar {
                label: month_start(k).format("%b").to_string(),
                amount: money(amt, code, 0),
                height: ((amt / max_month).to_f64().unwrap_or(0.0) * 100.0).clamp(3.0, 100.0),
                active: k == selected,
                href: format!("/?month={}-{:02}", k.0, k.1),
            }
        })
        .collect();

    // Net worth as of the end of the selected month (or the last directive,
    // whichever is earlier, so the current month isn't projected forward).
    let last_date = ledger.directives.last().map(|d| d.date());
    let nw_date = last_date.map(|d| d.min(end)).unwrap_or(end);
    let net_worth = net_worth_now(ledger, nw_date)
        .into_iter()
        .find(|c| c.prefix == prefix && c.suffix == suffix);
    let series = net_worth_series(ledger, currency);
    let net_trend = trend_since_first(&series, code);
    let sparkline = render_net_worth_sparkline(&series, code);

    let name = owner_name(ledger.meta.title.as_deref());
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());

    Home {
        greeting: greeting(now.hour()),
        initial,
        name,
        month_label: start.format("%B %Y").to_string(),
        months,
        gauge_svg: gauge_svg(progress),
        gauge_value: gauge_amount.to_f64().unwrap_or(0.0),
        gauge_text: money(gauge_amount, code, 0),
        gauge_caption,
        prefix,
        suffix,
        income_text: money(income, code, 0),
        spent_text: money(spent, code, 0),
        net_worth,
        net_trend,
        sparkline,
        savings_pct: format!("{:.0}%", (savings * 100.0).max(0.0)),
        savings_filled,
        savings_caption,
        txn_count,
        top_category,
        bars,
        bars_total: money(
            spent_by_month.get(&selected).copied().unwrap_or_default(),
            code,
            0,
        ),
        tiles,
        recent,
    }
}

/// A semicircular gauge (like a speedometer): a faint track, a bright arc
/// for the share spent, and a knob at its tip. The arc's draw-in animation
/// lives in CSS, keyed off the `--len` custom property set here.
fn gauge_svg(progress: f64) -> String {
    let len = std::f64::consts::PI * 90.0;
    let offset = len * (1.0 - progress);
    let angle = std::f64::consts::PI * (1.0 - progress);
    let (kx, ky) = (110.0 + 90.0 * angle.cos(), 110.0 - 90.0 * angle.sin());
    format!(
        r#"<svg viewBox="0 0 220 122" class="gauge" aria-hidden="true"><path d="M20 110 A90 90 0 0 1 200 110" class="gauge-track"/><path d="M20 110 A90 90 0 0 1 200 110" class="gauge-fill" style="stroke-dasharray:{len:.2};stroke-dashoffset:{offset:.2};--len:{len:.2}"/><circle cx="{kx:.1}" cy="{ky:.1}" r="8" class="gauge-knob"/></svg>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn fixture() -> Ledger {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../ob-core/tests/fixtures/anin_sample.beancount");
        ob_core::load(&path).ledger
    }

    fn noon() -> chrono::DateTime<chrono::Local> {
        chrono::Local
            .with_ymd_and_hms(2026, 1, 20, 12, 0, 0)
            .unwrap()
    }

    #[test]
    fn home_summarizes_the_latest_month() {
        let h = home(&fixture(), None, noon());
        assert_eq!(h.month_label, "January 2026");
        // Salary 50,000 + the 2.85 cashback folded into the rent settlement.
        assert_eq!(h.income_text, "₹50,003");
        // Rent 10,500.51 + Utilities 569.00 + Food 1,710.25
        assert_eq!(h.spent_text, "₹12,780");
        assert_eq!(h.gauge_text, "₹37,223");
        assert_eq!(h.name, "there", "the fixture title has no possessive name");
        assert_eq!(h.greeting, "Good afternoon");
        assert_eq!(h.tiles[0].label, "Housing");
        assert_eq!(h.txn_count, 4);
    }

    #[test]
    fn owner_name_reads_the_possessive_title() {
        assert_eq!(
            owner_name(Some("Anin's Personal Finance - (From May 2026)")),
            "Anin"
        );
        assert_eq!(owner_name(Some("Household books")), "there");
        assert_eq!(owner_name(None), "there");
    }

    #[test]
    fn unknown_month_falls_back_to_latest() {
        let h = home(&fixture(), Some("1999-01"), noon());
        assert_eq!(h.month_label, "January 2026");
    }

    #[test]
    fn defaults_past_transfer_only_months_and_keeps_gaps_in_the_timeline() {
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
        let h = home(&ob_core::load(&path).ledger, None, noon());
        assert_eq!(h.month_label, "January 2026", "March only has a transfer");
        let labels: Vec<_> = h.months.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Jan", "Feb", "Mar"],
            "February is a gap, not skipped"
        );
        assert_eq!(h.gauge_text, "₹900");
    }

    #[test]
    fn transfers_show_as_neutral_moves() {
        let h = home(&fixture(), None, noon());
        let card = h
            .recent
            .iter()
            .find(|a| a.title.contains("Card payment"))
            .unwrap();
        assert_eq!(card.emoji, "🔁");
        assert_eq!(card.amount, "₹12,777.00");
    }
}
