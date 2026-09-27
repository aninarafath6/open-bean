mod home;
mod reports;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use askama::Template;
use axum::Router;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use chrono::NaiveDate;
use clap::Parser;
use ob_core::LoadResult;

/// Self-hosted, Fava-style web UI for an open-bean ledger: read-only
/// reports (accounts, balance sheet, income statement, net worth) rendered
/// entirely server-side, no client-side JS at all.
#[derive(Parser)]
#[command(name = "ob-web", about = "Self-hosted web UI for an open-bean ledger")]
struct Cli {
    /// Path to the root .beancount file.
    ledger: PathBuf,
    #[arg(long, default_value = "127.0.0.1:3000")]
    bind: SocketAddr,
}

#[derive(Clone)]
struct AppState {
    root_path: Arc<PathBuf>,
}

impl AppState {
    /// Reloads and re-books the ledger from disk on every call. Simpler and
    /// always-fresh compared to mtime-based caching — a personal ledger
    /// parses and books in low milliseconds, and a self-hosted single-user
    /// app sees at most a handful of requests at a time, so there's nothing
    /// to gain yet from caching (revisit if that stops being true).
    fn load(&self) -> LoadResult {
        ob_core::load(&self.root_path)
    }
}

fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn parse_date(s: Option<&str>) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s?, "%Y-%m-%d").ok()
}

/// Renders every parse/booking error as plain (ANSI-free) prose so a broken
/// ledger still shows *something* useful instead of a blank error page.
fn render_errors(
    parse_errors: Vec<ob_core::ParseError>,
    booking_errors: Vec<ob_core::BookingError>,
) -> Vec<String> {
    parse_errors
        .into_iter()
        .map(|e| ob_diagnostics::render_narratable(&miette::Report::new(e)))
        .chain(
            booking_errors
                .into_iter()
                .map(|e| ob_diagnostics::render_narratable(&miette::Report::new(e))),
        )
        .collect()
}

fn render<T: Template>(tpl: T) -> Response {
    match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {err}"),
        )
            .into_response(),
    }
}

#[derive(Template)]
#[template(path = "home.html")]
struct HomeTemplate {
    nav: &'static str,
    errors: Vec<String>,
    home: home::Home,
}

#[derive(Template)]
#[template(path = "accounts.html")]
struct AccountsTemplate {
    nav: &'static str,
    errors: Vec<String>,
    page: reports::AccountsPage,
}

#[derive(Template)]
#[template(path = "account_detail.html")]
struct AccountDetailTemplate {
    nav: &'static str,
    errors: Vec<String>,
    detail: reports::AccountDetail,
}

#[derive(Template)]
#[template(path = "balance_sheet.html")]
struct BalanceSheetTemplate {
    nav: &'static str,
    errors: Vec<String>,
    sheet: reports::BalanceSheet,
}

#[derive(Template)]
#[template(path = "income_statement.html")]
struct IncomeStatementTemplate {
    nav: &'static str,
    errors: Vec<String>,
    stmt: reports::IncomeStatement,
}

#[derive(Template)]
#[template(path = "net_worth.html")]
struct NetWorthTemplate {
    nav: &'static str,
    errors: Vec<String>,
    page: reports::NetWorthPage,
}

#[derive(serde::Deserialize)]
struct MonthQuery {
    month: Option<String>,
}

async fn home_page(State(state): State<AppState>, Query(q): Query<MonthQuery>) -> Response {
    let LoadResult {
        ledger,
        parse_errors,
        booking_errors,
    } = state.load();
    let errors = render_errors(parse_errors, booking_errors);
    let home = home::home(
        &ledger,
        q.month.as_deref(),
        chrono::Local::now().naive_local(),
    );
    render(HomeTemplate {
        nav: "home",
        errors,
        home,
    })
}

async fn accounts_page(State(state): State<AppState>) -> Response {
    let LoadResult {
        ledger,
        parse_errors,
        booking_errors,
    } = state.load();
    let errors = render_errors(parse_errors, booking_errors);
    let as_of = ledger
        .directives
        .last()
        .map(|d| d.date())
        .unwrap_or_else(today);
    let page = reports::accounts_page(&ledger, as_of);
    render(AccountsTemplate {
        nav: "accounts",
        errors,
        page,
    })
}

async fn account_detail_page(
    State(state): State<AppState>,
    AxPath(name): AxPath<String>,
) -> Response {
    let LoadResult {
        ledger,
        parse_errors,
        booking_errors,
    } = state.load();
    let errors = render_errors(parse_errors, booking_errors);
    let account = ob_core::Account::new(&name);
    let detail = reports::account_detail(&ledger, account);
    render(AccountDetailTemplate {
        nav: "accounts",
        errors,
        detail,
    })
}

#[derive(serde::Deserialize)]
struct AsOfQuery {
    as_of: Option<String>,
}

async fn balance_sheet_page(State(state): State<AppState>, Query(q): Query<AsOfQuery>) -> Response {
    let LoadResult {
        ledger,
        parse_errors,
        booking_errors,
    } = state.load();
    let errors = render_errors(parse_errors, booking_errors);
    let default_as_of = ledger
        .directives
        .last()
        .map(|d| d.date())
        .unwrap_or_else(today);
    let as_of = parse_date(q.as_of.as_deref()).unwrap_or(default_as_of);
    let sheet = reports::balance_sheet(&ledger, as_of);
    render(BalanceSheetTemplate {
        nav: "balance",
        errors,
        sheet,
    })
}

#[derive(serde::Deserialize)]
struct RangeQuery {
    start: Option<String>,
    end: Option<String>,
}

async fn income_statement_page(
    State(state): State<AppState>,
    Query(q): Query<RangeQuery>,
) -> Response {
    let LoadResult {
        ledger,
        parse_errors,
        booking_errors,
    } = state.load();
    let errors = render_errors(parse_errors, booking_errors);
    let last = ledger
        .directives
        .last()
        .map(|d| d.date())
        .unwrap_or_else(today);
    let first = ledger.directives.first().map(|d| d.date()).unwrap_or(last);
    let start = parse_date(q.start.as_deref()).unwrap_or(first);
    let end = parse_date(q.end.as_deref()).unwrap_or(last);
    let stmt = reports::income_statement(&ledger, start, end);
    render(IncomeStatementTemplate {
        nav: "income",
        errors,
        stmt,
    })
}

async fn net_worth_page(State(state): State<AppState>) -> Response {
    let LoadResult {
        ledger,
        parse_errors,
        booking_errors,
    } = state.load();
    let errors = render_errors(parse_errors, booking_errors);
    let as_of = ledger
        .directives
        .last()
        .map(|d| d.date())
        .unwrap_or_else(today);
    let page = reports::net_worth_page(&ledger, as_of);
    render(NetWorthTemplate {
        nav: "trends",
        errors,
        page,
    })
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let state = AppState {
        root_path: Arc::new(cli.ledger),
    };

    let app = Router::new()
        .route("/", get(home_page))
        .route("/accounts", get(accounts_page))
        .route("/accounts/{name}", get(account_detail_page))
        .route("/reports/balance-sheet", get(balance_sheet_page))
        .route("/reports/income-statement", get(income_statement_page))
        .route("/reports/net-worth", get(net_worth_page))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(cli.bind)
        .await
        .expect("failed to bind address");
    println!("open-bean web listening on http://{}", cli.bind);
    axum::serve(listener, app).await.expect("server error");
}
