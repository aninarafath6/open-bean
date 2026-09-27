use chrono::NaiveDate;
use ob_diagnostics::{SourceFile, Span};
use rust_decimal::Decimal;
use ustr::Ustr;

/// Where a directive or posting came from, for diagnostics and for the web
/// UI's "jump to source" links. `SourceFile` is internally `Arc`-backed, so
/// cloning a `Loc` never copies file contents.
#[derive(Debug, Clone)]
pub struct Loc {
    pub source: SourceFile,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccountType {
    Assets,
    Liabilities,
    Equity,
    Income,
    Expenses,
}

impl AccountType {
    pub fn from_root(root: &str) -> Option<Self> {
        match root {
            "Assets" => Some(Self::Assets),
            "Liabilities" => Some(Self::Liabilities),
            "Equity" => Some(Self::Equity),
            "Income" => Some(Self::Income),
            "Expenses" => Some(Self::Expenses),
            _ => None,
        }
    }

    /// Assets/Expenses grow with debits (positive postings); the other three
    /// grow with credits. Reports use this to decide sign conventions.
    pub fn normal_balance_is_debit(self) -> bool {
        matches!(self, AccountType::Assets | AccountType::Expenses)
    }
}

/// A `:`-separated account path, interned so it's `Copy` and cheap to use as
/// a hash-map key across a whole ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Account(pub Ustr);

impl Account {
    pub fn new(full: &str) -> Self {
        Self(Ustr::from(full))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn root_str(&self) -> &str {
        self.0.split(':').next().unwrap_or_default()
    }

    pub fn root_type(&self) -> Option<AccountType> {
        AccountType::from_root(self.root_str())
    }

    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split(':')
    }
}

impl std::fmt::Display for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Amount {
    pub number: Decimal,
    pub currency: Ustr,
}

impl std::fmt::Display for Amount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.number, self.currency)
    }
}

/// A per-account, per-currency running balance. No lot/cost tracking in v1 —
/// see the "explicitly deferred" note in the engine architecture plan.
pub type Inventory = std::collections::BTreeMap<Ustr, Decimal>;

#[derive(Debug, Clone)]
pub struct Posting {
    pub account: Account,
    /// Always `Some` after booking — a single elided posting per transaction
    /// is interpolated in place; see `booking::book_transaction`.
    pub amount: Option<Amount>,
    pub flag: Option<char>,
    pub meta: Vec<(String, String)>,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Transaction {
    pub date: NaiveDate,
    pub flag: char,
    pub payee: Option<String>,
    pub narration: Option<String>,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub meta: Vec<(String, String)>,
    pub postings: Vec<Posting>,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Open {
    pub date: NaiveDate,
    pub account: Account,
    pub currencies: Vec<Ustr>,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Close {
    pub date: NaiveDate,
    pub account: Account,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Balance {
    pub date: NaiveDate,
    pub account: Account,
    pub amount: Amount,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Pad {
    pub date: NaiveDate,
    pub account: Account,
    pub source_account: Account,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Price {
    pub date: NaiveDate,
    pub currency: Ustr,
    pub amount: Amount,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub date: NaiveDate,
    pub name: String,
    pub value: String,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Note {
    pub date: NaiveDate,
    pub account: Account,
    pub comment: String,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Document {
    pub date: NaiveDate,
    pub account: Account,
    pub path: String,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub struct Commodity {
    pub date: NaiveDate,
    pub currency: Ustr,
    pub loc: Loc,
}

/// A parsed `custom` directive value. `ob-budget` reads these to find
/// `custom "budget" ...` directives without ob-core needing to know what a
/// budget is.
#[derive(Debug, Clone)]
pub enum CustomValue {
    Str(String),
    Date(NaiveDate),
    Amount(Amount),
    Account(Account),
    Currency(Ustr),
    Bool(bool),
}

#[derive(Debug, Clone)]
pub struct Custom {
    pub date: NaiveDate,
    pub name: String,
    pub values: Vec<CustomValue>,
    pub loc: Loc,
}

#[derive(Debug, Clone)]
pub enum Directive {
    Open(Open),
    Close(Close),
    Balance(Balance),
    Pad(Pad),
    Price(Price),
    Event(Event),
    Note(Note),
    Document(Document),
    Commodity(Commodity),
    Custom(Custom),
    Transaction(Transaction),
}

impl Directive {
    pub fn date(&self) -> NaiveDate {
        match self {
            Directive::Open(d) => d.date,
            Directive::Close(d) => d.date,
            Directive::Balance(d) => d.date,
            Directive::Pad(d) => d.date,
            Directive::Price(d) => d.date,
            Directive::Event(d) => d.date,
            Directive::Note(d) => d.date,
            Directive::Document(d) => d.date,
            Directive::Commodity(d) => d.date,
            Directive::Custom(d) => d.date,
            Directive::Transaction(d) => d.date,
        }
    }

    pub fn loc(&self) -> &Loc {
        match self {
            Directive::Open(d) => &d.loc,
            Directive::Close(d) => &d.loc,
            Directive::Balance(d) => &d.loc,
            Directive::Pad(d) => &d.loc,
            Directive::Price(d) => &d.loc,
            Directive::Event(d) => &d.loc,
            Directive::Note(d) => &d.loc,
            Directive::Document(d) => &d.loc,
            Directive::Commodity(d) => &d.loc,
            Directive::Custom(d) => &d.loc,
            Directive::Transaction(d) => &d.loc,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct LedgerMeta {
    pub title: Option<String>,
    pub operating_currency: Option<String>,
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct Ledger {
    /// Sorted by `(date, source order)` — same-day entries keep the order
    /// they appeared in the (possibly multi-file, `include`-merged) source.
    pub directives: Vec<Directive>,
    pub meta: LedgerMeta,
}
