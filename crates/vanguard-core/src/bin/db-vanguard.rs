//! `db-vanguard` CLI — single-binary pure-Rust SQL toolkit, PostgreSQL v3 wire client,
//! SQLite 3 header inspector, SQL formatter/safety linter, and constant-memory streaming exporter.

use clap::{Parser, Subcommand};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use vanguard_core::drivers::local_engine::{inspect_sqlite_file, LocalEngine};
use vanguard_core::drivers::postgres_wire::PostgresWireDriver;
use vanguard_core::drivers::DatabaseDriver;
use vanguard_core::exporter::{
    benchmark_streaming_export, stream_export_rows, ExportFormat, ExportOptions,
};
use vanguard_core::plugins::load_plugins_from_dir;
use vanguard_core::sql_parser::{format_sql, lint_sql, LintSeverity};
use vanguard_core::{ConnectionProfile, QueryResult, CORE_VERSION};

#[derive(Parser)]
#[command(
    name = "db-vanguard",
    version = CORE_VERSION,
    about = "Native pure-Rust database CLI, SQL formatter/linter, SQLite 3 inspector, and constant-memory streaming exporter."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a SQL query against the built-in demo engine or a PostgreSQL v3 wire endpoint.
    Query {
        /// SQL query string to execute.
        sql: String,
        /// Output format: table, json, or csv.
        #[arg(long, default_value = "table")]
        format: String,
        /// Target driver: local or postgres.
        #[arg(long, default_value = "local")]
        driver: String,
        /// PostgreSQL host (used when --driver postgres).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// PostgreSQL port (used when --driver postgres).
        #[arg(long, default_value_t = 5432)]
        port: u16,
        /// Target database name.
        #[arg(long, default_value = "postgres")]
        database: String,
        /// Database username (used when --driver postgres).
        #[arg(long, default_value = "postgres")]
        user: String,
        /// Database password (optional; prefer PGPASSWORD env var).
        #[arg(long)]
        password: Option<String>,
    },
    /// Pretty-format SQL using the zero-dependency tokenizer and formatter.
    Format {
        /// SQL string to format (ignored if --file is set).
        sql: Option<String>,
        /// Read SQL input from a file path.
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Run destructive-query safety linting (unbounded DELETE/UPDATE, DROP, TRUNCATE, SELECT *).
    Lint {
        /// SQL string to lint (ignored if --file is set).
        sql: Option<String>,
        /// Read SQL input from a file path.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Emit diagnostics as JSON.
        #[arg(long)]
        json: bool,
        /// Exit with status code 2 if any Error-severity lint issue is found.
        #[arg(long)]
        fail_on_error: bool,
    },
    /// Inspect a binary SQLite 3 database file header (page size, WAL flags, schema version, B-tree page 1).
    InspectSqlite {
        /// Path to the `.sqlite` or `.db` file.
        file: PathBuf,
        /// Emit header inspection as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stream query results directly to CSV (OWASP formula-safe), NDJSON, or SQL INSERT with constant memory.
    Export {
        /// SQL SELECT statement to stream.
        #[arg(long)]
        query: String,
        /// Export format: csv, ndjson, or sql.
        #[arg(long, default_value = "csv")]
        format: String,
        /// Output file path (use '-' for stdout).
        #[arg(long, default_value = "-")]
        output: String,
        /// Target table name for SQL INSERT exports.
        #[arg(long, default_value = "exported_rows")]
        table: String,
        /// Rows per multi-row INSERT statement when --format sql.
        #[arg(long, default_value_t = 500)]
        batch_size: usize,
        /// Disable OWASP CSV formula-injection neutralization.
        #[arg(long)]
        no_formula_guard: bool,
    },
    /// Benchmark constant-memory streaming export throughput across N generated transactional rows.
    BenchExport {
        /// Number of rows to generate and stream.
        #[arg(long, default_value_t = 50_000)]
        rows: usize,
        /// Export format: csv, ndjson, or sql.
        #[arg(long, default_value = "csv")]
        format: String,
        /// Optional output path (defaults to in-memory sink for pure throughput benchmark).
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Load and validate all community database driver plugin manifests in `plugins/*.json`.
    ValidatePlugins {
        /// Directory containing driver manifest JSON files.
        #[arg(long, default_value = "plugins")]
        dir: PathBuf,
    },
    /// List tables or describe a specific table in the built-in e-commerce demo engine.
    Tables {
        /// Optional table name to inspect column schema.
        #[arg(long)]
        describe: Option<String>,
    },
    /// Print binary and core engine version details.
    Version,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn std::error::Error>> {
    match cli.command {
        Commands::Version => {
            println!("db-vanguard {CORE_VERSION} (vanguard-core {CORE_VERSION}, zero-C-deps)");
            Ok(ExitCode::SUCCESS)
        }
        Commands::Format { sql, file } => {
            let input = read_sql_arg(sql, file)?;
            println!("{}", format_sql(&input));
            Ok(ExitCode::SUCCESS)
        }
        Commands::Lint {
            sql,
            file,
            json,
            fail_on_error,
        } => {
            let input = read_sql_arg(sql, file)?;
            let issues = lint_sql(&input);
            let has_error = issues.iter().any(|i| i.severity == LintSeverity::Error);

            if json {
                println!("{}", serde_json::to_string_pretty(&issues)?);
            } else if issues.is_empty() {
                println!("PASS: 0 lint issues found.");
            } else {
                for issue in &issues {
                    let level = match issue.severity {
                        LintSeverity::Error => "ERROR",
                        LintSeverity::Warning => "WARN ",
                        LintSeverity::Info => "INFO ",
                    };
                    println!(
                        "[{level}] {} (stmt #{}, bytes {}..{}): {}",
                        issue.code, issue.statement_index, issue.start, issue.end, issue.message
                    );
                    println!("        suggestion: {}", issue.suggestion);
                }
                println!("Summary: {} issue(s) detected.", issues.len());
            }

            if fail_on_error && has_error {
                Ok(ExitCode::from(2))
            } else {
                Ok(ExitCode::SUCCESS)
            }
        }
        Commands::InspectSqlite { file, json } => {
            let info = inspect_sqlite_file(&file)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&info)?);
            } else {
                println!("SQLite 3 Binary Header Inspection: {}", file.display());
                println!("  SQLite Version   : {} ({})", info.sqlite_version, info.sqlite_version_raw);
                println!("  Page Size        : {} bytes", info.page_size);
                println!("  Page Count       : {}", info.page_count);
                println!("  Estimated Size   : {} bytes", info.estimated_db_bytes);
                println!("  Journal Mode     : {}", info.journal_mode_hint);
                println!("  Text Encoding    : {}", info.text_encoding);
                println!("  Schema Format    : {}", info.schema_format);
                println!("  Schema Cookie    : {}", info.schema_cookie);
                println!("  Change Counter   : {}", info.file_change_counter);
                println!("  Freelist Pages   : {}", info.freelist_page_count);
                if let Some(btree) = &info.page1_btree_type {
                    println!(
                        "  Page 1 B-Tree    : {} ({} schema cells)",
                        btree,
                        info.page1_cell_count.unwrap_or(0)
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::Query {
            sql,
            format,
            driver,
            host,
            port,
            database,
            user,
            password,
        } => {
            let result = match driver.to_ascii_lowercase().as_str() {
                "local" => {
                    let mut engine = LocalEngine::with_ecommerce_demo();
                    engine.execute_query(&sql)?
                }
                "postgres" | "pg" => {
                    let pw = password.or_else(|| std::env::var("PGPASSWORD").ok());
                    let profile = ConnectionProfile {
                        id: "cli-pg".to_string(),
                        name: "CLI PostgreSQL".to_string(),
                        driver: "postgres-wire-v3".to_string(),
                        host: Some(host),
                        port: Some(port),
                        database,
                        username: Some(user),
                        password: pw,
                        read_only: false,
                    };
                    let mut pg = PostgresWireDriver::new();
                    pg.connect(&profile)?;
                    pg.execute_query(&sql)?
                }
                other => {
                    return Err(format!("unsupported driver '{other}'; use local or postgres").into());
                }
            };

            render_query_result(&result, &format)?;
            Ok(ExitCode::SUCCESS)
        }
        Commands::Export {
            query,
            format,
            output,
            table,
            batch_size,
            no_formula_guard,
        } => {
            let export_fmt = ExportFormat::parse(&format)?;
            let options = ExportOptions {
                format: export_fmt,
                table_name: table,
                batch_size,
                neutralize_formulas: !no_formula_guard,
                include_header: true,
            };
            let mut engine = LocalEngine::with_ecommerce_demo();

            let stats = if output == "-" {
                let mut stdout = io::stdout().lock();
                engine.stream_export(&query, &options, &mut stdout)?
            } else {
                let mut file = File::create(&output)?;
                engine.stream_export(&query, &options, &mut file)?
            };

            if output != "-" {
                println!(
                    "Exported {} rows ({} bytes) as {:?} in {:.2} ms ({:.0} rows/sec) -> {}",
                    stats.rows_exported,
                    stats.bytes_written,
                    stats.format,
                    stats.duration_ms,
                    stats.rows_per_second,
                    output
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Commands::BenchExport {
            rows,
            format,
            output,
        } => {
            let export_fmt = ExportFormat::parse(&format)?;
            let stats = if let Some(path) = output {
                let file = File::create(&path)?;
                let stats = benchmark_streaming_export(rows, export_fmt, file)?;
                println!("Output Path  : {}", path.display());
                stats
            } else {
                benchmark_streaming_export(rows, export_fmt, io::sink())?
            };

            println!("Streaming Export Benchmark ({:?})", stats.format);
            println!("  Rows Streamed : {}", stats.rows_exported);
            println!("  Bytes Written : {} ({:.2} MB)", stats.bytes_written, stats.bytes_written as f64 / (1024.0 * 1024.0));
            println!("  Elapsed Time  : {:.2} ms", stats.duration_ms);
            println!("  Throughput    : {:.0} rows/sec", stats.rows_per_second);
            println!("  Buffer Size   : 64 KiB fixed BufWriter (O(1) memory)");
            Ok(ExitCode::SUCCESS)
        }
        Commands::ValidatePlugins { dir } => {
            let (manifests, reports) = load_plugins_from_dir(&dir)?;
            let total = reports.len();
            let passed = manifests.len();
            for rep in &reports {
                if rep.valid {
                    println!(
                        "PASS  {} ({})",
                        rep.file,
                        rep.plugin_id.as_deref().unwrap_or("unknown")
                    );
                } else {
                    println!("FAIL  {}: {}", rep.file, rep.errors.join("; "));
                }
            }
            println!("Plugin validation: {passed}/{total} PASS");
            if passed == total && total > 0 {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::FAILURE)
            }
        }
        Commands::Tables { describe } => {
            let mut engine = LocalEngine::with_ecommerce_demo();
            if let Some(table_name) = describe {
                let schema = engine.describe_table(&table_name)?;
                println!(
                    "Table {}.{} ({} rows)",
                    schema.schema, schema.name, schema.row_estimate
                );
                for c in schema.columns {
                    let pk = if c.primary_key { " PRIMARY KEY" } else { "" };
                    let nullability = if c.nullable { "NULL" } else { "NOT NULL" };
                    println!("  - {:<18} {:<12} {}{}", c.name, c.data_type, nullability, pk);
                }
            } else {
                let tables = engine.list_tables()?;
                for t in tables {
                    println!(
                        "{}.{:<16} {:>4} cols  {:>5} rows",
                        t.schema,
                        t.name,
                        t.columns.len(),
                        t.row_estimate
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn read_sql_arg(sql: Option<String>, file: Option<PathBuf>) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(path) = file {
        Ok(fs::read_to_string(path)?)
    } else if let Some(inline) = sql {
        Ok(inline)
    } else {
        Err("provide either an inline SQL argument or --file <path>".into())
    }
}

fn render_query_result(result: &QueryResult, format: &str) -> Result<(), Box<dyn std::error::Error>> {
    match format.to_ascii_lowercase().as_str() {
        "json" => {
            println!("{}", serde_json::to_string_pretty(result)?);
        }
        "csv" => {
            let mut stdout = io::stdout().lock();
            stream_export_rows(
                &result.columns,
                result.rows.clone(),
                &ExportOptions::default(),
                &mut stdout,
            )?;
            stdout.flush()?;
        }
        "table" => {
            let mut widths: Vec<usize> = result.columns.iter().map(|c| c.name.len()).collect();
            let rendered_rows: Vec<Vec<String>> = result
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .enumerate()
                        .map(|(i, v)| {
                            let s = v.display_string();
                            if i < widths.len() && s.len() > widths[i] {
                                widths[i] = s.len();
                            }
                            s
                        })
                        .collect()
                })
                .collect();

            let sep: String = widths
                .iter()
                .map(|w| "-".repeat(w + 2))
                .collect::<Vec<_>>()
                .join("+");
            println!("+{sep}+");
            let header: String = result
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| format!(" {:<width$} ", c.name, width = widths[i]))
                .collect::<Vec<_>>()
                .join("|");
            println!("|{header}|");
            println!("+{sep}+");
            for row in &rendered_rows {
                let line: String = row
                    .iter()
                    .enumerate()
                    .map(|(i, cell)| format!(" {:<width$} ", cell, width = widths[i]))
                    .collect::<Vec<_>>()
                    .join("|");
                println!("|{line}|");
            }
            println!("+{sep}+");
            println!(
                "{} | {} row(s) | {:.3} ms",
                result.command_tag,
                result.rows.len(),
                result.execution_ms
            );
        }
        other => {
            return Err(format!("unsupported format '{other}'; expected table, json, or csv").into());
        }
    }
    Ok(())
}
