//! Source-spanned AST produced by the parser. Deliberately free of semantic
//! knowledge: account root types, currency validity, booking, and arithmetic
//! all belong to `ob-core`. This layer only knows how to shape tokens into
//! directives.

use ob_diagnostics::Span;
use rust_decimal::Decimal;

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    pub value: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(value: T, span: Span) -> Self {
        Self { value, span }
    }
}

/// A dotted account path exactly as written (`Assets:Anin:Kotak:Checking`),
/// split into its `:`-separated segments. `ob-core` derives the root type
/// (Assets/Liabilities/Equity/Income/Expenses) from `segments[0]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Account {
    pub segments: Vec<String>,
}

impl Account {
    pub fn from_colon_path(path: &str) -> Self {
        Self {
            segments: path.split(':').map(str::to_string).collect(),
        }
    }
}

impl std::fmt::Display for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.segments.join(":"))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Amount {
    pub number: Decimal,
    pub currency: String,
}

/// A parsed-but-not-yet-interpreted `{...}` cost specification on a posting.
/// Beancount's lot-booking semantics (STRICT/FIFO/AVERAGE reduction) are
/// explicitly deferred past v1; we still parse the syntax so files that use
/// it don't fail to load.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CostSpec {
    pub per_unit: Option<Amount>,
    pub date: Option<String>,
    pub label: Option<String>,
}

/// A parsed-but-not-yet-interpreted `@ price` / `@@ total-price` annotation.
#[derive(Debug, Clone, PartialEq)]
pub enum PriceAnnotation {
    PerUnit(Amount),
    Total(Amount),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metadata {
    pub key: String,
    pub raw_value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Posting {
    pub flag: Option<char>,
    pub account: Account,
    pub amount: Option<Amount>,
    pub cost: Option<CostSpec>,
    pub price: Option<PriceAnnotation>,
    pub meta: Vec<Metadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transaction {
    pub date: String,
    pub flag: char,
    pub payee: Option<String>,
    pub narration: Option<String>,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub meta: Vec<Metadata>,
    pub postings: Vec<Spanned<Posting>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CustomValue {
    Str(String),
    Date(String),
    Amount(Amount),
    Account(Account),
    Currency(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    Option {
        name: String,
        value: String,
    },
    Include {
        path: String,
    },
    Plugin {
        module: String,
        config: Option<String>,
    },
    PushTag(String),
    PopTag(String),
    Open {
        date: String,
        account: Account,
        currencies: Vec<String>,
        booking: Option<String>,
    },
    Close {
        date: String,
        account: Account,
    },
    Commodity {
        date: String,
        currency: String,
        meta: Vec<Metadata>,
    },
    Balance {
        date: String,
        account: Account,
        amount: Amount,
    },
    Pad {
        date: String,
        account: Account,
        source_account: Account,
    },
    Price {
        date: String,
        currency: String,
        amount: Amount,
    },
    Event {
        date: String,
        name: String,
        value: String,
    },
    Note {
        date: String,
        account: Account,
        comment: String,
    },
    Document {
        date: String,
        account: Account,
        path: String,
    },
    Query {
        date: String,
        name: String,
        query_string: String,
    },
    Custom {
        date: String,
        name: String,
        values: Vec<CustomValue>,
    },
    Transaction(Transaction),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ledger {
    pub directives: Vec<Spanned<Directive>>,
}
