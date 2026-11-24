import {
  COMMUNITY_DRIVER_PLUGINS,
  LocalBrowserEngine,
  formatSql,
  lintSql,
  tokenizeSql,
} from "./sqlEngine";
import type {
  ConnectionProfile,
  DriverPluginManifest,
  LintIssue,
  QueryResult,
  TableSchema,
  TokenSpan,
} from "./types";

interface TauriInternals {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
}

function getTauri(): TauriInternals | null {
  const w = window as unknown as { __TAURI_INTERNALS__?: TauriInternals };
  return w.__TAURI_INTERNALS__ ?? null;
}

const browserEngine = new LocalBrowserEngine();

export const DEFAULT_PROFILES: ConnectionProfile[] = [
  {
    id: "local-demo",
    name: "Local Demo",
    driver: "local-engine",
    host: null,
    port: null,
    database: "vanguard_ecommerce_demo",
    username: null,
    read_only: false,
  },
  {
    id: "sqlite-local",
    name: "SQLite",
    driver: "sqlite-wal-native",
    host: null,
    port: null,
    database: "./data/ledger.sqlite",
    username: null,
    read_only: false,
  },
  {
    id: "postgres-local",
    name: "PostgreSQL",
    driver: "postgres-wire-v3",
    host: "127.0.0.1",
    port: 5432,
    database: "production_replica",
    username: "postgres",
    read_only: true,
  },
  {
    id: "mysql-local",
    name: "MySQL",
    driver: "mysql-protocol-v8",
    host: "127.0.0.1",
    port: 3306,
    database: "billing_v8",
    username: "readonly",
    read_only: true,
  },
];

export const isDesktopRuntime = (): boolean => getTauri() !== null;

export async function connectProfile(profile: ConnectionProfile): Promise<void> {
  const tauri = getTauri();
  if (tauri) {
    await tauri.invoke("connect_profile", { profile });
  }
}

export async function listTables(): Promise<TableSchema[]> {
  const tauri = getTauri();
  if (tauri) {
    return tauri.invoke<TableSchema[]>("list_tables");
  }
  return browserEngine.listTables();
}

export async function executeSql(sql: string): Promise<QueryResult> {
  const tauri = getTauri();
  if (tauri) {
    return tauri.invoke<QueryResult>("execute_sql", { sql });
  }
  return browserEngine.executeQuery(sql);
}

export async function formatSqlAsync(sql: string): Promise<string> {
  const tauri = getTauri();
  if (tauri) {
    return tauri.invoke<string>("format_sql", { sql });
  }
  return formatSql(sql);
}

export function lintSqlSync(sql: string): LintIssue[] {
  return lintSql(sql);
}

export function tokenizeSqlSync(sql: string): TokenSpan[] {
  return tokenizeSql(sql);
}

export async function listPlugins(): Promise<DriverPluginManifest[]> {
  const tauri = getTauri();
  if (tauri) {
    try {
      return await tauri.invoke<DriverPluginManifest[]>("list_plugins", {});
    } catch {
      return COMMUNITY_DRIVER_PLUGINS;
    }
  }
  return COMMUNITY_DRIVER_PLUGINS;
}
