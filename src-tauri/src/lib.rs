//! Tauri v2 desktop bridge exposing `vanguard-core` pure-Rust SQL parsing,
//! formatting, safety linting, local/PostgreSQL drivers, and streaming exports.

use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::State;
use vanguard_core::drivers::local_engine::{
    inspect_sqlite_file, LocalEngine, SqliteHeaderInfo,
};
use vanguard_core::drivers::postgres_wire::PostgresWireDriver;
use vanguard_core::drivers::DatabaseDriver;
use vanguard_core::exporter::{ExportFormat, ExportOptions, ExportStats};
use vanguard_core::plugins::{load_plugins_from_dir, DriverPluginManifest};
use vanguard_core::sql_parser::{
    format_sql as core_format_sql, lint_sql as core_lint_sql, tokenize_sql as core_tokenize_sql,
    LintIssue, TokenSpan,
};
use vanguard_core::{ConnectionProfile, QueryResult, TableSchema};

/// Active session state shared safely across Tauri IPC commands.
pub struct AppState {
    driver: Mutex<Box<dyn DatabaseDriver>>,
    active_profile: Mutex<ConnectionProfile>,
}

impl Default for AppState {
    fn default() -> Self {
        let profile = ConnectionProfile {
            id: "local-default".to_string(),
            name: "Local E-Commerce Demo".to_string(),
            driver: "local-engine".to_string(),
            host: None,
            port: None,
            database: "vanguard_ecommerce_demo".to_string(),
            username: None,
            password: None,
            read_only: false,
        };
        Self {
            driver: Mutex::new(Box::new(LocalEngine::with_ecommerce_demo())),
            active_profile: Mutex::new(profile),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionStatus {
    pub profile_id: String,
    pub driver_id: String,
    pub database: String,
    pub connected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamExportRequest {
    pub sql: String,
    pub format: String,
    pub output_path: String,
    pub table_name: Option<String>,
    pub batch_size: Option<usize>,
    pub neutralize_formulas: Option<bool>,
}

#[tauri::command]
pub fn connect_profile(
    profile: ConnectionProfile,
    state: State<'_, AppState>,
) -> Result<ConnectionStatus, String> {
    let mut new_driver: Box<dyn DatabaseDriver> = match profile.driver.as_str() {
        "postgres-wire-v3" | "postgres" | "postgresql" => Box::new(PostgresWireDriver::new()),
        _ => Box::new(LocalEngine::with_ecommerce_demo()),
    };

    new_driver.connect(&profile).map_err(|e| e.to_string())?;
    let status = ConnectionStatus {
        profile_id: profile.id.clone(),
        driver_id: new_driver.driver_id().to_string(),
        database: profile.database.clone(),
        connected: new_driver.is_connected(),
    };

    *state.driver.lock().map_err(|e| e.to_string())? = new_driver;
    *state.active_profile.lock().map_err(|e| e.to_string())? = profile;

    Ok(status)
}

#[tauri::command]
pub fn list_tables(state: State<'_, AppState>) -> Result<Vec<TableSchema>, String> {
    let mut guard = state.driver.lock().map_err(|e| e.to_string())?;
    guard.list_tables().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn describe_table(table_name: String, state: State<'_, AppState>) -> Result<TableSchema, String> {
    let mut guard = state.driver.lock().map_err(|e| e.to_string())?;
    guard.describe_table(&table_name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn tokenize_sql(sql: String) -> Vec<TokenSpan> {
    core_tokenize_sql(&sql)
}

#[tauri::command]
pub fn format_sql(sql: String) -> String {
    core_format_sql(&sql)
}

#[tauri::command]
pub fn lint_sql(sql: String) -> Vec<LintIssue> {
    core_lint_sql(&sql)
}

#[tauri::command]
pub fn execute_sql(sql: String, state: State<'_, AppState>) -> Result<QueryResult, String> {
    let mut guard = state.driver.lock().map_err(|e| e.to_string())?;
    guard.execute_query(&sql).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn stream_export(
    request: StreamExportRequest,
    state: State<'_, AppState>,
) -> Result<ExportStats, String> {
    let format = ExportFormat::parse(&request.format).map_err(|e| e.to_string())?;
    let options = ExportOptions {
        format,
        table_name: request
            .table_name
            .unwrap_or_else(|| "exported_rows".to_string()),
        batch_size: request.batch_size.unwrap_or(500),
        neutralize_formulas: request.neutralize_formulas.unwrap_or(true),
        include_header: true,
    };

    let mut file = File::create(Path::new(&request.output_path)).map_err(|e| e.to_string())?;
    let mut guard = state.driver.lock().map_err(|e| e.to_string())?;
    guard
        .stream_export(&request.sql, &options, &mut file)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_plugins(plugins_dir: Option<String>) -> Result<Vec<DriverPluginManifest>, String> {
    let path = plugins_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../plugins"));
    let (manifests, _reports) = load_plugins_from_dir(&path).map_err(|e| e.to_string())?;
    Ok(manifests)
}

#[tauri::command]
pub fn inspect_sqlite_header(file_path: String) -> Result<SqliteHeaderInfo, String> {
    inspect_sqlite_file(Path::new(&file_path)).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            connect_profile,
            list_tables,
            describe_table,
            tokenize_sql,
            format_sql,
            lint_sql,
            execute_sql,
            stream_export,
            list_plugins,
            inspect_sqlite_header
        ])
        .run(tauri::generate_context!())
        .expect("error while running db-vanguard desktop application");
}
