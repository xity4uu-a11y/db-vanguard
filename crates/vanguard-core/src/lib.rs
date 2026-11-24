//! Core engine for db-vanguard: zero-dependency SQL lexer/parser/formatter/linter,
//! pluggable database drivers (PostgreSQL v3 wire protocol, local relational SQL engine,
//! SQLite 3 binary header inspector), constant-memory streaming exporter, and driver plugin loader.

pub mod drivers;
pub mod exporter;
pub mod plugins;
pub mod sql_parser;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Semantic version of `vanguard-core`.
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Typed database cell value shared across drivers, query results, and streaming exporters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(String),
}

impl Value {
    /// Returns true if the value is SQL NULL.
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Formats the value for human-readable tabular display.
    #[must_use]
    pub fn display_string(&self) -> String {
        match self {
            Self::Null => "NULL".to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Integer(i) => i.to_string(),
            Self::Float(f) => {
                if f.fract() == 0.0 && f.is_finite() {
                    format!("{f:.1}")
                } else {
                    format!("{f:.2}")
                }
            }
            Self::Text(s) => s.clone(),
        }
    }

    /// Attempts to coerce the value to an `f64` for numeric ordering and aggregates.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            Self::Text(s) => s.parse::<f64>().ok(),
            Self::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Self::Null => None,
        }
    }

    /// Compares two values using SQL-like ordering (NULL sorts last in ascending order).
    #[must_use]
    pub fn sql_cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Null, _) => Ordering::Greater,
            (_, Self::Null) => Ordering::Less,
            (Self::Integer(a), Self::Integer(b)) => a.cmp(b),
            (Self::Float(a), Self::Float(b)) => a.total_cmp(b),
            (Self::Integer(a), Self::Float(b)) => (*a as f64).total_cmp(b),
            (Self::Float(a), Self::Integer(b)) => a.total_cmp(&(*b as f64)),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
            _ => self.display_string().cmp(&other.display_string()),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display_string())
    }
}

/// Column metadata returned alongside a query result set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

/// Complete tabular result returned by a driver execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryResult {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    pub affected_rows: u64,
    pub execution_ms: f64,
    pub command_tag: String,
}

/// Column definition within a table schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnSchema {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub default_value: Option<String>,
}

/// Table metadata returned by `list_tables` and `describe_table`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSchema {
    pub name: String,
    pub schema: String,
    pub row_estimate: u64,
    pub columns: Vec<ColumnSchema>,
}

/// Connection configuration profile for a database target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: String,
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub read_only: bool,
}

/// Unified error type for vanguard-core operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbError {
    Connection(String),
    Authentication(String),
    Protocol(String),
    Syntax(String),
    Execution(String),
    NotFound(String),
    Io(String),
    Validation(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connection(msg) => write!(f, "connection error: {msg}"),
            Self::Authentication(msg) => write!(f, "authentication error: {msg}"),
            Self::Protocol(msg) => write!(f, "protocol error: {msg}"),
            Self::Syntax(msg) => write!(f, "syntax error: {msg}"),
            Self::Execution(msg) => write!(f, "execution error: {msg}"),
            Self::NotFound(msg) => write!(f, "not found: {msg}"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::Validation(msg) => write!(f, "validation error: {msg}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<std::io::Error> for DbError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}
