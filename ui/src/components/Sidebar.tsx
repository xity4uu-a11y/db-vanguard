import { useState } from "react";
import type { ConnectionProfile, TableSchema } from "../lib/types";

interface SidebarProps {
  profiles: ConnectionProfile[];
  activeProfile: ConnectionProfile;
  tables: TableSchema[];
  onSelectProfile: (profile: ConnectionProfile) => void;
  onSelectQuery: (sql: string) => void;
}

const PRESET_QUERIES = [
  {
    label: "Top Paid Orders + Customer Join",
    sql: `SELECT
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
LIMIT 50;`,
  },
  {
    label: "Revenue by Order Status",
    sql: `SELECT
  status,
  COUNT(*) AS order_count,
  SUM(total_cents) AS gross_cents,
  AVG(total_cents) AS avg_order_cents
FROM orders
GROUP BY status
ORDER BY gross_cents DESC;`,
  },
  {
    label: "Execution Plan (EXPLAIN)",
    sql: "EXPLAIN SELECT id, email, plan FROM users WHERE active = TRUE ORDER BY id DESC LIMIT 25;",
  },
  {
    label: "Test Destructive Query Linter",
    sql: "DELETE FROM orders;",
  },
];

export function Sidebar({
  profiles,
  activeProfile,
  tables,
  onSelectProfile,
  onSelectQuery,
}: SidebarProps) {
  const [expanded, setExpanded] = useState<Record<string, boolean>>({
    orders: true,
    users: false,
  });

  const toggleTable = (name: string) => {
    setExpanded((prev) => ({ ...prev, [name]: !prev[name] }));
  };

  return (
    <aside className="sidebar" aria-label="Connections and Schema Explorer">
      <div className="sidebar-section">
        <div className="section-label">
          <span>Connections</span>
          <span>{profiles.length}</span>
        </div>
        <div className="conn-list">
          {profiles.map((profile) => {
            const active = profile.id === activeProfile.id;
            return (
              <button
                key={profile.id}
                type="button"
                className={`conn-item ${active ? "active" : ""}`}
                onClick={() => onSelectProfile(profile)}
              >
                <div>
                  <div>{profile.name}</div>
                  <span className="conn-sub mono">{profile.database}</span>
                </div>
                <span className="tag">{profile.read_only ? "RO" : "RW"}</span>
              </button>
            );
          })}
        </div>
      </div>

      <div className="sidebar-section">
        <div className="section-label">
          <span>Schema ({tables.length} Tables)</span>
          <span>public</span>
        </div>
        <div className="table-tree">
          {tables.map((table) => {
            const isOpen = Boolean(expanded[table.name]);
            return (
              <div key={table.name}>
                <button
                  type="button"
                  className="table-row"
                  onClick={() => toggleTable(table.name)}
                  onDoubleClick={() =>
                    onSelectQuery(`SELECT\n  ${table.columns.map((c) => c.name).join(",\n  ")}\nFROM ${table.name}\nLIMIT 100;`)
                  }
                  title="Click to inspect columns, double-click to query top 100 rows"
                >
                  <span className="table-row-left">
                    <span className="mono" style={{ fontSize: 10, color: "var(--text-muted)" }}>
                      {isOpen ? "v" : ">"}
                    </span>
                    <span className="mono">{table.name}</span>
                  </span>
                  <span className="table-count">{table.row_estimate}</span>
                </button>
                {isOpen && (
                  <div className="col-list">
                    {table.columns.map((col) => (
                      <div key={col.name} className="col-item">
                        <span className="mono">
                          {col.name}
                          {col.primary_key ? " [PK]" : ""}
                        </span>
                        <span className="col-type">{col.data_type}</span>
                      </div>
                    ))}
                    <button
                      type="button"
                      className="btn btn-ghost"
                      style={{ height: 22, fontSize: 11, justifyContent: "flex-start", padding: "0 4px" }}
                      onClick={() =>
                        onSelectQuery(
                          `SELECT\n  ${table.columns.map((c) => c.name).join(",\n  ")}\nFROM ${table.name}\nLIMIT 100;`
                        )
                      }
                    >
                      Select top 100 rows
                    </button>
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </div>

      <div className="sidebar-section" style={{ borderBottom: "none" }}>
        <div className="section-label">
          <span>Quick Queries</span>
        </div>
        <div className="preset-list">
          {PRESET_QUERIES.map((preset) => (
            <button
              key={preset.label}
              type="button"
              className="preset-btn"
              onClick={() => onSelectQuery(preset.sql)}
            >
              {preset.label}
            </button>
          ))}
        </div>
      </div>
    </aside>
  );
}
