import { useCallback, useEffect, useMemo, useState } from "react";
import { DataGrid } from "./components/DataGrid";
import { DriversDrawer } from "./components/DriversDrawer";
import { ExportModal } from "./components/ExportModal";
import { Sidebar } from "./components/Sidebar";
import { SqlEditor } from "./components/SqlEditor";
import {
  DEFAULT_PROFILES,
  connectProfile,
  executeSql,
  formatSqlAsync,
  isDesktopRuntime,
  lintSqlSync,
  listPlugins,
  listTables,
} from "./lib/tauriBridge";
import type {
  ConnectionProfile,
  DriverPluginManifest,
  QueryResult,
  TableSchema,
} from "./lib/types";

const INITIAL_SQL = `SELECT
  o.id,
  u.name,
  u.email,
  o.region,
  o.total_cents,
  o.status
FROM orders o
INNER JOIN users u ON o.user_id = u.id
WHERE o.status = 'paid'
ORDER BY o.total_cents DESC
LIMIT 50;`;

export function App() {
  const [theme, setTheme] = useState<"light" | "dark">("light");
  const [activeProfile, setActiveProfile] = useState<ConnectionProfile>(DEFAULT_PROFILES[0]);
  const [tables, setTables] = useState<TableSchema[]>([]);
  const [plugins, setPlugins] = useState<DriverPluginManifest[]>([]);
  const [sql, setSql] = useState<string>(INITIAL_SQL);
  const [result, setResult] = useState<QueryResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [showExport, setShowExport] = useState(false);
  const [showDrivers, setShowDrivers] = useState(false);

  const lintIssues = useMemo(() => lintSqlSync(sql), [sql]);

  useEffect(() => {
    document.documentElement.setAttribute("data-theme", theme);
  }, [theme]);

  const refreshMetadata = useCallback(async () => {
    try {
      const [tbls, plugs] = await Promise.all([listTables(), listPlugins()]);
      setTables(tbls);
      setPlugins(plugs);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const runQuery = useCallback(
    async (queryText: string, forceDestructive = false) => {
      const issues = lintSqlSync(queryText);
      const hasBlocking = issues.some((i) => i.severity === "error");
      if (hasBlocking && !forceDestructive) {
        setError(
          "Execution blocked by Safety Linter: destructive SQL detected. Review the Safety Linter banner or click 'Run Anyway (Destructive)'."
        );
        return;
      }

      setIsRunning(true);
      setError(null);
      try {
        const res = await executeSql(queryText);
        setResult(res);
        await refreshMetadata();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setIsRunning(false);
      }
    },
    [refreshMetadata]
  );

  useEffect(() => {
    void refreshMetadata();
    void runQuery(INITIAL_SQL, true);
  }, [refreshMetadata, runQuery]);

  const handleSelectProfile = async (profile: ConnectionProfile) => {
    setActiveProfile(profile);
    setError(null);
    try {
      await connectProfile(profile);
      await refreshMetadata();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleFormat = async () => {
    const formatted = await formatSqlAsync(sql);
    setSql(formatted);
  };

  const handleExplain = () => {
    const clean = sql.trim().replace(/^EXPLAIN\s+/i, "");
    const explained = `EXPLAIN ${clean}`;
    setSql(explained);
    void runQuery(explained, true);
  };

  const handleSelectPreset = (nextSql: string) => {
    setSql(nextSql);
    const hasBlocking = lintSqlSync(nextSql).some((i) => i.severity === "error");
    if (!hasBlocking) {
      void runQuery(nextSql, false);
    } else {
      setError(null);
    }
  };

  return (
    <div className="studio-shell">
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark mono">DV</span>
          <span className="brand-title">db-vanguard</span>
          <span className="brand-meta">
            Native SQL Studio | {isDesktopRuntime() ? "Tauri v2 Core" : "Local Engine"}
          </span>
        </div>

        <div className="topbar-actions">
          <span className="pill-status">
            <span className="status-dot" />
            <span>
              {activeProfile.name} ({activeProfile.driver})
            </span>
          </span>
          <button type="button" className="btn" onClick={() => setShowDrivers(true)}>
            Drivers ({plugins.length})
          </button>
          <button
            type="button"
            className="btn"
            onClick={() => setTheme((prev) => (prev === "light" ? "dark" : "light"))}
          >
            {theme === "light" ? "Dark Mode" : "Light Mode"}
          </button>
        </div>
      </header>

      <main className="workspace">
        <Sidebar
          profiles={DEFAULT_PROFILES}
          activeProfile={activeProfile}
          tables={tables}
          onSelectProfile={(p) => void handleSelectProfile(p)}
          onSelectQuery={handleSelectPreset}
        />

        <div className="main-split">
          <SqlEditor
            sql={sql}
            lintIssues={lintIssues}
            isRunning={isRunning}
            onChange={setSql}
            onRun={(force) => void runQuery(sql, force)}
            onFormat={() => void handleFormat()}
            onExplain={handleExplain}
            onOpenExport={() => setShowExport(true)}
          />

          <DataGrid
            result={result}
            error={error}
            onOpenExport={() => setShowExport(true)}
          />
        </div>
      </main>

      {showExport && <ExportModal result={result} onClose={() => setShowExport(false)} />}
      {showDrivers && <DriversDrawer plugins={plugins} onClose={() => setShowDrivers(false)} />}
    </div>
  );
}
