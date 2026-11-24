//! Constant-memory streaming exporter supporting CSV (with OWASP formula-injection
//! neutralization), NDJSON, and multi-row batched SQL INSERT statements.

use crate::{ColumnMeta, DbError, Value};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{BufWriter, Write};
use std::time::Instant;

/// Supported streaming export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Ndjson,
    Sql,
}

impl ExportFormat {
    /// Parses a CLI or UI format identifier string.
    pub fn parse(raw: &str) -> Result<Self, DbError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(Self::Csv),
            "ndjson" | "jsonl" => Ok(Self::Ndjson),
            "sql" | "sql-insert" => Ok(Self::Sql),
            other => Err(DbError::Validation(format!(
                "unsupported export format '{other}'; expected csv, ndjson, or sql"
            ))),
        }
    }
}

/// Configuration for constant-memory streaming exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub table_name: String,
    pub batch_size: usize,
    pub neutralize_formulas: bool,
    pub include_header: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: ExportFormat::Csv,
            table_name: "exported_rows".to_string(),
            batch_size: 500,
            neutralize_formulas: true,
            include_header: true,
        }
    }
}

/// Telemetry returned upon completion of a streaming export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportStats {
    pub format: ExportFormat,
    pub rows_exported: u64,
    pub bytes_written: u64,
    pub duration_ms: f64,
    pub rows_per_second: f64,
}

struct CountingWriter<W: Write> {
    inner: W,
    bytes_written: u64,
}

impl<W: Write> CountingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            bytes_written: 0,
        }
    }
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.bytes_written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Applies OWASP CSV Formula Injection neutralization to a text cell.
///
/// Any string starting with `=`, `+`, `-`, `@`, `\t`, or `\r` (including after leading
/// ASCII spaces) is prefixed with a leading single quote `'` so spreadsheet applications
/// do not execute it as a formula or DDE command.
#[must_use]
pub fn neutralize_csv_formula(cell: &str) -> Cow<'_, str> {
    let first_non_space = cell.trim_start_matches(' ');
    let dangerous = cell
        .as_bytes()
        .first()
        .is_some_and(|b| matches!(b, b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r'))
        || first_non_space
            .as_bytes()
            .first()
            .is_some_and(|b| matches!(b, b'=' | b'+' | b'-' | b'@' | b'\t' | b'\r'));

    if dangerous {
        let mut safe = String::with_capacity(cell.len() + 1);
        safe.push('\'');
        safe.push_str(cell);
        Cow::Owned(safe)
    } else {
        Cow::Borrowed(cell)
    }
}

fn write_csv_cell<W: Write>(writer: &mut W, raw: &str) -> std::io::Result<()> {
    let needs_quotes = raw.contains([',', '"', '\n', '\r']);
    if !needs_quotes {
        writer.write_all(raw.as_bytes())
    } else {
        writer.write_all(b"\"")?;
        for ch in raw.chars() {
            if ch == '"' {
                writer.write_all(b"\"\"")?;
            } else {
                let mut buf = [0u8; 4];
                writer.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
            }
        }
        writer.write_all(b"\"")
    }
}

fn quote_sql_identifier(ident: &str) -> String {
    let escaped = ident.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

fn write_sql_value<W: Write>(writer: &mut W, val: &Value) -> std::io::Result<()> {
    match val {
        Value::Null => writer.write_all(b"NULL"),
        Value::Bool(true) => writer.write_all(b"TRUE"),
        Value::Bool(false) => writer.write_all(b"FALSE"),
        Value::Integer(i) => write!(writer, "{i}"),
        Value::Float(f) => {
            if f.is_finite() {
                write!(writer, "{f}")
            } else {
                writer.write_all(b"NULL")
            }
        }
        Value::Text(s) => {
            writer.write_all(b"'")?;
            for ch in s.chars() {
                if ch == '\'' {
                    writer.write_all(b"''")?;
                } else {
                    let mut buf = [0u8; 4];
                    writer.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
                }
            }
            writer.write_all(b"'")
        }
    }
}

/// Streams rows one-by-one into `writer` through a fixed 64 KiB `BufWriter`,
/// guaranteeing O(1) memory overhead regardless of dataset cardinality.
pub fn stream_export_rows<W, I>(
    columns: &[ColumnMeta],
    rows: I,
    options: &ExportOptions,
    writer: W,
) -> Result<ExportStats, DbError>
where
    W: Write,
    I: IntoIterator<Item = Vec<Value>>,
{
    let start = Instant::now();
    let counting = CountingWriter::new(writer);
    let mut buf = BufWriter::with_capacity(64 * 1024, counting);
    let mut rows_exported: u64 = 0;

    match options.format {
        ExportFormat::Csv => {
            if options.include_header {
                for (idx, col) in columns.iter().enumerate() {
                    if idx > 0 {
                        buf.write_all(b",")?;
                    }
                    let header = if options.neutralize_formulas {
                        neutralize_csv_formula(&col.name)
                    } else {
                        Cow::Borrowed(col.name.as_str())
                    };
                    write_csv_cell(&mut buf, &header)?;
                }
                buf.write_all(b"\n")?;
            }

            for row in rows {
                for (idx, val) in row.iter().enumerate() {
                    if idx > 0 {
                        buf.write_all(b",")?;
                    }
                    match val {
                        Value::Null => {}
                        Value::Bool(b) => {
                            buf.write_all(if *b { b"true" } else { b"false" })?;
                        }
                        Value::Integer(i) => write!(buf, "{i}")?,
                        Value::Float(f) => write!(buf, "{f}")?,
                        Value::Text(s) => {
                            let safe = if options.neutralize_formulas {
                                neutralize_csv_formula(s)
                            } else {
                                Cow::Borrowed(s.as_str())
                            };
                            write_csv_cell(&mut buf, &safe)?;
                        }
                    }
                }
                buf.write_all(b"\n")?;
                rows_exported += 1;
            }
        }
        ExportFormat::Ndjson => {
            for row in rows {
                buf.write_all(b"{")?;
                for (idx, col) in columns.iter().enumerate() {
                    if idx > 0 {
                        buf.write_all(b",")?;
                    }
                    serde_json::to_writer(&mut buf, &col.name)
                        .map_err(|e| DbError::Io(e.to_string()))?;
                    buf.write_all(b":")?;
                    let val = row.get(idx).unwrap_or(&Value::Null);
                    serde_json::to_writer(&mut buf, val)
                        .map_err(|e| DbError::Io(e.to_string()))?;
                }
                buf.write_all(b"}\n")?;
                rows_exported += 1;
            }
        }
        ExportFormat::Sql => {
            let batch_limit = options.batch_size.max(1);
            let table_ident = quote_sql_identifier(if options.table_name.trim().is_empty() {
                "exported_rows"
            } else {
                &options.table_name
            });
            let col_list: String = columns
                .iter()
                .map(|c| quote_sql_identifier(&c.name))
                .collect::<Vec<_>>()
                .join(", ");

            let mut in_batch: usize = 0;

            for row in rows {
                if in_batch == 0 {
                    write!(buf, "INSERT INTO {table_ident} ({col_list}) VALUES\n  (")?;
                } else {
                    buf.write_all(b",\n  (")?;
                }

                for (idx, val) in row.iter().enumerate() {
                    if idx > 0 {
                        buf.write_all(b", ")?;
                    }
                    write_sql_value(&mut buf, val)?;
                }
                buf.write_all(b")")?;

                in_batch += 1;
                rows_exported += 1;

                if in_batch >= batch_limit {
                    buf.write_all(b";\n")?;
                    in_batch = 0;
                }
            }

            if in_batch > 0 {
                buf.write_all(b";\n")?;
            }
        }
    }

    buf.flush()?;
    let inner = buf
        .into_inner()
        .map_err(|e| DbError::Io(e.into_error().to_string()))?;

    let elapsed = start.elapsed();
    let duration_ms = elapsed.as_secs_f64() * 1000.0;
    let rows_per_second = if duration_ms > 0.0 {
        (rows_exported as f64) / (duration_ms / 1000.0)
    } else {
        rows_exported as f64
    };

    Ok(ExportStats {
        format: options.format,
        rows_exported,
        bytes_written: inner.bytes_written,
        duration_ms,
        rows_per_second,
    })
}

/// Benchmarks constant-memory streaming export by lazily generating `row_count` realistic
/// transactional rows and streaming them straight into `writer` without buffering the table.
pub fn benchmark_streaming_export<W: Write>(
    row_count: usize,
    format: ExportFormat,
    writer: W,
) -> Result<ExportStats, DbError> {
    let columns = vec![
        ColumnMeta {
            name: "order_id".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: false,
        },
        ColumnMeta {
            name: "customer_email".to_string(),
            data_type: "TEXT".to_string(),
            nullable: false,
        },
        ColumnMeta {
            name: "region".to_string(),
            data_type: "TEXT".to_string(),
            nullable: false,
        },
        ColumnMeta {
            name: "status".to_string(),
            data_type: "TEXT".to_string(),
            nullable: false,
        },
        ColumnMeta {
            name: "total_cents".to_string(),
            data_type: "INTEGER".to_string(),
            nullable: false,
        },
        ColumnMeta {
            name: "created_at".to_string(),
            data_type: "TIMESTAMP".to_string(),
            nullable: false,
        },
    ];

    const REGIONS: [&str; 4] = ["us-east", "us-west", "eu-central", "ap-northeast"];
    const STATUSES: [&str; 4] = ["paid", "shipped", "processing", "refunded"];

    let lazy_rows = (1..=row_count).map(|idx| {
        let region = REGIONS[idx % REGIONS.len()];
        let status = STATUSES[(idx / 3) % STATUSES.len()];
        let cents = ((idx as i64 * 7919) % 48_500) + 1_500;
        let day = (idx % 28) + 1;
        let hour = idx % 24;
        vec![
            Value::Integer(idx as i64),
            Value::Text(format!("customer{idx}@example.org")),
            Value::Text(region.to_string()),
            Value::Text(status.to_string()),
            Value::Integer(cents),
            Value::Text(format!("2026-10-{day:02}T{hour:02}:15:00Z")),
        ]
    });

    let options = ExportOptions {
        format,
        table_name: "orders".to_string(),
        batch_size: 500,
        neutralize_formulas: true,
        include_header: true,
    };

    stream_export_rows(&columns, lazy_rows, &options, writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
            },
            ColumnMeta {
                name: "note".to_string(),
                data_type: "TEXT".to_string(),
                nullable: true,
            },
            ColumnMeta {
                name: "amount".to_string(),
                data_type: "REAL".to_string(),
                nullable: false,
            },
        ]
    }

    #[test]
    fn csv_neutralizes_owasp_formula_injection_prefixes() {
        assert_eq!(neutralize_csv_formula("=SUM(A1:A2)"), "'=SUM(A1:A2)");
        assert_eq!(neutralize_csv_formula("+cmd|' /C calc'!A0"), "'+cmd|' /C calc'!A0");
        assert_eq!(neutralize_csv_formula("-2+3"), "'-2+3");
        assert_eq!(neutralize_csv_formula("@HYPERLINK(\"http://x\")"), "'@HYPERLINK(\"http://x\")");
        assert_eq!(neutralize_csv_formula("\t=1+1"), "'\t=1+1");
        assert_eq!(neutralize_csv_formula("\r=1+1"), "'\r=1+1");
        assert_eq!(neutralize_csv_formula("  =1+1"), "'  =1+1");
        assert_eq!(neutralize_csv_formula("Normal invoice note"), "Normal invoice note");
    }

    #[test]
    fn streams_csv_with_rfc4180_escaping_and_formula_guard() {
        let cols = sample_columns();
        let rows = vec![
            vec![
                Value::Integer(1),
                Value::Text("=HYPERLINK(\"https://evil.test\",\"click\")".to_string()),
                Value::Float(19.95),
            ],
            vec![
                Value::Integer(2),
                Value::Text("Plain, \"quoted\" text".to_string()),
                Value::Float(-4.5),
            ],
            vec![Value::Integer(3), Value::Null, Value::Float(0.0)],
        ];

        let mut out = Vec::new();
        let stats = stream_export_rows(&cols, rows, &ExportOptions::default(), &mut out).unwrap();
        assert_eq!(stats.rows_exported, 3);

        let csv = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "id,note,amount");
        assert_eq!(
            lines[1],
            "1,\"'=HYPERLINK(\"\"https://evil.test\"\",\"\"click\"\")\",19.95"
        );
        assert_eq!(lines[2], "2,\"Plain, \"\"quoted\"\" text\",-4.5");
        assert_eq!(lines[3], "3,,0");
    }

    #[test]
    fn streams_ndjson_valid_json_per_line() {
        let cols = sample_columns();
        let rows = vec![
            vec![
                Value::Integer(10),
                Value::Text("alpha".to_string()),
                Value::Float(99.5),
            ],
            vec![Value::Integer(11), Value::Null, Value::Float(12.0)],
        ];

        let mut out = Vec::new();
        let opts = ExportOptions {
            format: ExportFormat::Ndjson,
            ..ExportOptions::default()
        };
        let stats = stream_export_rows(&cols, rows, &opts, &mut out).unwrap();
        assert_eq!(stats.rows_exported, 2);

        let text = String::from_utf8(out).unwrap();
        let parsed: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["id"], 10);
        assert_eq!(parsed[0]["note"], "alpha");
        assert!(parsed[1]["note"].is_null());
    }

    #[test]
    fn streams_batched_sql_inserts_with_escaping() {
        let cols = sample_columns();
        let rows = vec![
            vec![
                Value::Integer(1),
                Value::Text("O'Reilly".to_string()),
                Value::Float(49.0),
            ],
            vec![
                Value::Integer(2),
                Value::Text("Line".to_string()),
                Value::Float(10.0),
            ],
            vec![Value::Integer(3), Value::Null, Value::Float(0.0)],
        ];

        let mut out = Vec::new();
        let opts = ExportOptions {
            format: ExportFormat::Sql,
            table_name: "invoices".to_string(),
            batch_size: 2,
            ..ExportOptions::default()
        };
        let stats = stream_export_rows(&cols, rows, &opts, &mut out).unwrap();
        assert_eq!(stats.rows_exported, 3);

        let sql = String::from_utf8(out).unwrap();
        assert_eq!(sql.matches("INSERT INTO \"invoices\"").count(), 2);
        assert!(sql.contains("'O''Reilly'"));
        assert!(sql.contains("(3, NULL, 0)"));
    }

    #[test]
    fn benchmarks_50000_rows_constant_memory_streaming_export() {
        let stats = benchmark_streaming_export(50_000, ExportFormat::Csv, std::io::sink()).unwrap();
        assert_eq!(stats.rows_exported, 50_000);
        assert!(stats.bytes_written > 2_000_000);
        assert!(
            stats.duration_ms < 5_000.0,
            "50k row export took too long: {:.2}ms",
            stats.duration_ms
        );
    }
}
