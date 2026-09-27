use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use ob_core::query;

#[derive(Parser)]
#[command(
    name = "ob",
    version,
    about = "A from-scratch beancount-compatible ledger engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse, book, and validate a ledger, reporting every problem found.
    Check { path: PathBuf },
    /// Print the balance of every open account (as of the last directive,
    /// or `--as-of YYYY-MM-DD`).
    Balances {
        path: PathBuf,
        #[arg(long)]
        as_of: Option<String>,
    },
    /// Print every posting to one account, in date order.
    Query { path: PathBuf, account: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { path } => cmd_check(&path),
        Command::Balances { path, as_of } => cmd_balances(&path, as_of.as_deref()),
        Command::Query { path, account } => cmd_query(&path, &account),
    }
}

fn report_all(parse_errors: Vec<ob_core::ParseError>, booking_errors: Vec<ob_core::BookingError>) {
    for err in parse_errors {
        eprintln!(
            "{}",
            ob_diagnostics::render_graphical(&miette::Report::new(err))
        );
    }
    for err in booking_errors {
        eprintln!(
            "{}",
            ob_diagnostics::render_graphical(&miette::Report::new(err))
        );
    }
}

fn cmd_check(path: &Path) -> ExitCode {
    let result = ob_core::load(path);
    let is_ok = result.is_ok();
    let directive_count = result.ledger.directives.len();
    let error_count = result.parse_errors.len() + result.booking_errors.len();
    report_all(result.parse_errors, result.booking_errors);

    if is_ok {
        println!("ok: {directive_count} directives booked, 0 errors");
        ExitCode::SUCCESS
    } else {
        eprintln!("failed: {error_count} error(s)");
        ExitCode::FAILURE
    }
}

fn cmd_balances(path: &Path, as_of: Option<&str>) -> ExitCode {
    let result = ob_core::load(path);
    if !result.is_ok() {
        report_all(result.parse_errors, result.booking_errors);
        eprintln!("(showing balances anyway, from what did book successfully)");
    }

    let as_of_date = match as_of {
        Some(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => {
                eprintln!("invalid --as-of date `{s}`, expected YYYY-MM-DD");
                return ExitCode::FAILURE;
            }
        },
        None => result
            .ledger
            .directives
            .last()
            .map(|d| d.date())
            .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
    };

    for account in query::open_accounts(&result.ledger) {
        let inventory = query::balance_at(&result.ledger, account, as_of_date);
        for (currency, amount) in &inventory {
            if !amount.is_zero() {
                println!("{:<40} {:>15} {}", account, amount, currency);
            }
        }
    }
    ExitCode::SUCCESS
}

fn cmd_query(path: &Path, account_name: &str) -> ExitCode {
    let result = ob_core::load(path);
    if !result.is_ok() {
        report_all(result.parse_errors, result.booking_errors);
    }

    let account = ob_core::Account::new(account_name);
    for (txn, posting) in query::postings_for_account(&result.ledger, account) {
        let amount = posting.amount.expect("booked");
        let narration = txn.narration.as_deref().unwrap_or("");
        println!("{}  {:>15}  {}", txn.date, amount, narration);
    }
    ExitCode::SUCCESS
}
