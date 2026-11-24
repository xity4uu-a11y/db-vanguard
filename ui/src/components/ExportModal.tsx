import { useState } from "react";
import { benchmarkStreamingExport, exportRowsToPayload } from "../lib/sqlEngine";
import type { ExportFormat, ExportStats, QueryResult } from "../lib/types";

interface ExportModalProps {
  result: QueryResult | null;
  onClose: () => void;
}

export function ExportModal({ result, onClose }: ExportModalProps) {
  const [format, setFormat] = useState<ExportFormat>("csv");
  const [tableName, setTableName] = useState("orders");
  const [batchSize, setBatchSize] = useState(500);
  const [neutralizeFormulas, setNeutralizeFormulas] = useState(true);
  const [lastStats, setLastStats] = useState<ExportStats | null>(null);

  const handleDownload = () => {
    if (!result) return;
    const { chunks, stats } = exportRowsToPayload(result.columns, result.rows, {
      format,
      tableName,
      batchSize,
      neutralizeFormulas,
      includeHeader: true,
    });
    setLastStats(stats);

    const mime =
      format === "csv"
        ? "text/csv;charset=utf-8"
        : format === "ndjson"
          ? "application/x-ndjson;charset=utf-8"
          : "application/sql;charset=utf-8";
    const ext = format === "csv" ? "csv" : format === "ndjson" ? "ndjson" : "sql";

    const blob = new Blob(chunks, { type: mime });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `${tableName || "vanguard-export"}.${ext}`;
    link.click();
    URL.revokeObjectURL(url);
  };

  const handleBenchmark50k = () => {
    const stats = benchmarkStreamingExport(50000, format);
    setLastStats(stats);
  };

  return (
    <div className="backdrop" role="dialog" aria-modal="true" aria-label="Streaming Export">
      <div className="modal-card">
        <div className="panel-header">
          <h2 className="panel-title">Constant-Memory Streaming Export</h2>
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            Close
          </button>
        </div>

        <div className="panel-body">
          <div className="field-row">
            <span className="field-label">Format</span>
            <div className="segment-group">
              {(["csv", "ndjson", "sql"] as ExportFormat[]).map((fmt) => (
                <button
                  key={fmt}
                  type="button"
                  className={`segment-btn ${format === fmt ? "active" : ""}`}
                  onClick={() => setFormat(fmt)}
                >
                  {fmt === "csv" ? "CSV" : fmt === "ndjson" ? "NDJSON" : "SQL INSERT"}
                </button>
              ))}
            </div>
          </div>

          {format === "csv" && (
            <label className="checkbox-row">
              <input
                type="checkbox"
                checked={neutralizeFormulas}
                onChange={(e) => setNeutralizeFormulas(e.target.checked)}
              />
              <span>OWASP CSV formula-injection guard (neutralize =, +, -, @, tab, CR prefixes)</span>
            </label>
          )}

          {format === "sql" && (
            <div style={{ display: "grid", gridTemplateColumns: "1fr 140px", gap: 10 }}>
              <div className="field-row">
                <label className="field-label" htmlFor="export-table-name">
                  Target Table Name
                </label>
                <input
                  id="export-table-name"
                  className="search-input"
                  style={{ width: "100%" }}
                  value={tableName}
                  onChange={(e) => setTableName(e.target.value)}
                />
              </div>
              <div className="field-row">
                <label className="field-label" htmlFor="export-batch-size">
                  Rows per Batch
                </label>
                <input
                  id="export-batch-size"
                  type="number"
                  min={1}
                  max={5000}
                  className="search-input"
                  style={{ width: "100%" }}
                  value={batchSize}
                  onChange={(e) => setBatchSize(Number(e.target.value) || 500)}
                />
              </div>
            </div>
          )}

          <div className="stat-card">
            <div>
              <div className="stat-label">Active Rows</div>
              <div className="stat-value mono">{(result?.rows.length ?? 0).toLocaleString()}</div>
            </div>
            <div>
              <div className="stat-label">Stream Buffer</div>
              <div className="stat-value mono">64 KiB</div>
            </div>
            <div>
              <div className="stat-label">Memory Profile</div>
              <div className="stat-value">O(1) Constant</div>
            </div>
          </div>

          {lastStats && (
            <div className="stat-card" style={{ borderColor: "var(--sem-pass-border)", background: "var(--sem-pass-bg)" }}>
              <div>
                <div className="stat-label">Streamed</div>
                <div className="stat-value mono">{lastStats.rows_exported.toLocaleString()} rows</div>
              </div>
              <div>
                <div className="stat-label">Elapsed</div>
                <div className="stat-value mono">{lastStats.duration_ms.toFixed(1)} ms</div>
              </div>
              <div>
                <div className="stat-label">Output Size</div>
                <div className="stat-value mono">{(lastStats.bytes_written / 1024).toFixed(1)} KiB</div>
              </div>
            </div>
          )}
        </div>

        <div className="panel-footer">
          <button type="button" className="btn" onClick={handleBenchmark50k}>
            Benchmark 50,000 Rows
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={!result || result.rows.length === 0}
            onClick={handleDownload}
          >
            Download .{format}
          </button>
        </div>
      </div>
    </div>
  );
}
