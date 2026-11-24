//! Pluggable database driver abstraction and built-in pure-Rust drivers.

pub mod local_engine;
pub mod postgres_wire;

use crate::exporter::{ExportOptions, ExportStats};
use crate::{ConnectionProfile, DbError, QueryResult, TableSchema};
use std::io::Write;

/// Object-safe database driver contract implemented by all native and community drivers.
pub trait DatabaseDriver: Send {
    /// Human-readable driver identifier (e.g., `"local-engine"`, `"postgres-wire-v3"`).
    fn driver_id(&self) -> &'static str;

    /// Establishes a session against the target connection profile.
    fn connect(&mut self, profile: &ConnectionProfile) -> Result<(), DbError>;

    /// Returns true if the driver has an active session.
    fn is_connected(&self) -> bool;

    /// Lists all accessible tables with column definitions and row count estimates.
    fn list_tables(&mut self) -> Result<Vec<TableSchema>, DbError>;

    /// Returns detailed schema metadata for a single table.
    fn describe_table(&mut self, table_name: &str) -> Result<TableSchema, DbError>;

    /// Executes a SQL statement and returns tabular rows, affected row count, and timing.
    fn execute_query(&mut self, sql: &str) -> Result<QueryResult, DbError>;

    /// Executes `sql` and streams all result rows directly to `writer` with constant memory.
    fn stream_export(
        &mut self,
        sql: &str,
        options: &ExportOptions,
        writer: &mut dyn Write,
    ) -> Result<ExportStats, DbError>;
}
