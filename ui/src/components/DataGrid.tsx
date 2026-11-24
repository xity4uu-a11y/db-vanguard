import { useMemo, useState } from "react";
import { paginateRows, sortRows } from "../lib/sqlEngine";
import type { CellValue, QueryResult, SortState } from "../lib/types";

interface DataGridProps {
  result: QueryResult | null;
  error: string | null;
  onOpenExport: () => void;
}

export function DataGrid({ result, error, onOpenExport }: DataGridProps) {
  const [sortState, setSortState] = useState<SortState | null>(null);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(50);
  const [filterText, setFilterText] = useState("");

  const filteredRows = useMemo(() => {
    if (!result) return [];
    const query = filterText.trim().toLowerCase();
    if (!query) return result.rows;
    return result.rows.filter((row) =>
      row.some((cell) => cell !== null && String(cell).toLowerCase().includes(query))
    );
  }, [result, filterText]);

  const sortedRows = useMemo(() => {
    if (!sortState) return filteredRows;
    return sortRows(filteredRows, sortState.columnIndex, sortState.direction);
  }, [filteredRows, sortState]);

  const pageSlice = useMemo(
    () => paginateRows(sortedRows, page, pageSize),
    [sortedRows, page, pageSize]
  );

  const toggleSort = (colIdx: number) => {
    setPage(1);
    setSortState((prev) => {
      if (!prev || prev.columnIndex !== colIdx) {
        return { columnIndex: colIdx, direction: "asc" };
      }
      if (prev.direction === "asc") {
        return { columnIndex: colIdx, direction: "desc" };
      }
      return null;
    });
  };

  const renderCell = (val: CellValue) => {
    if (val === null) {
      return <span className="cell-null">NULL</span>;
    }
    if (typeof val === "boolean") {
      return <span className="cell-bool">{val ? "true" : "false"}</span>;
    }
    if (typeof val === "number") {
      return <span className="mono">{val.toLocaleString()}</span>;
    }
    return <span>{val}</span>;
  };

  return (
    <section className="grid-pane" aria-label="Query Results Data Grid">
      <div className="grid-toolbar">
        <div className="grid-metrics">
          {result ? (
            <>
              <span>
                Rows: <span className="metric-strong">{result.rows.length.toLocaleString()}</span>
              </span>
              <span>
                Time: <span className="metric-strong mono">{result.execution_ms.toFixed(2)} ms</span>
              </span>
              <span className="tag mono">{result.command_tag}</span>
            </>
          ) : (
            <span>Execute a query to inspect results</span>
          )}
        </div>

        <div className="toolbar-group">
          <input
            type="search"
            className="search-input"
            value={filterText}
            onChange={(e) => {
              setFilterText(e.target.value);
              setPage(1);
            }}
            placeholder="Filter visible rows..."
            aria-label="Filter result rows"
          />
          <button
            type="button"
            className="btn"
            disabled={!result || result.rows.length === 0}
            onClick={onOpenExport}
          >
            Export ({result?.rows.length ?? 0})
          </button>
        </div>
      </div>

      <div className="grid-scroll">
        {error ? (
          <div className="error-box" role="alert">
            <strong>Execution Error:</strong> {error}
          </div>
        ) : !result || result.columns.length === 0 ? (
          <div style={{ padding: 28, color: "var(--text-muted)", textAlign: "center" }}>
            No rows returned.
          </div>
        ) : (
          <table className="data-table">
            <thead>
              <tr>
                <th style={{ width: 48, textAlign: "right", cursor: "default" }}>#</th>
                {result.columns.map((col, idx) => {
                  const isSorted = sortState?.columnIndex === idx;
                  const indicator = isSorted ? (sortState.direction === "asc" ? " [ASC]" : " [DESC]") : "";
                  return (
                    <th key={`${col.name}-${idx}`} onClick={() => toggleSort(idx)}>
                      <div className="th-inner">
                        <span>
                          {col.name}
                          {indicator}
                        </span>
                        <span className="th-type">{col.data_type}</span>
                      </div>
                    </th>
                  );
                })}
              </tr>
            </thead>
            <tbody>
              {pageSlice.items.map((row, rowOffset) => (
                <tr key={pageSlice.startIndex + rowOffset}>
                  <td className="cell-num" style={{ color: "var(--text-muted)" }}>
                    {pageSlice.startIndex + rowOffset + 1}
                  </td>
                  {row.map((cell, cIdx) => (
                    <td key={cIdx} className={typeof cell === "number" ? "cell-num" : undefined}>
                      {renderCell(cell)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="grid-footer">
        <div>
          Showing{" "}
          <span className="mono">
            {pageSlice.totalItems === 0 ? 0 : pageSlice.startIndex + 1}-{pageSlice.endIndex}
          </span>{" "}
          of <span className="mono">{pageSlice.totalItems.toLocaleString()}</span> rows
        </div>

        <div className="toolbar-group">
          <label htmlFor="page-size-select" style={{ color: "var(--text-muted)" }}>
            Page size
          </label>
          <select
            id="page-size-select"
            className="select-compact"
            value={pageSize}
            onChange={(e) => {
              setPageSize(Number(e.target.value));
              setPage(1);
            }}
          >
            <option value={25}>25</option>
            <option value={50}>50</option>
            <option value={100}>100</option>
            <option value={250}>250</option>
          </select>

          <button
            type="button"
            className="btn"
            disabled={pageSlice.page <= 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            Previous
          </button>
          <span className="mono">
            {pageSlice.page} / {pageSlice.totalPages}
          </span>
          <button
            type="button"
            className="btn"
            disabled={pageSlice.page >= pageSlice.totalPages}
            onClick={() => setPage((p) => Math.min(pageSlice.totalPages, p + 1))}
          >
            Next
          </button>
        </div>
      </div>
    </section>
  );
}
