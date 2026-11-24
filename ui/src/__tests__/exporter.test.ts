import { describe, expect, it } from "vitest";
import {
  benchmarkStreamingExport,
  exportRowsToPayload,
  neutralizeCsvFormula,
} from "../lib/sqlEngine";
import type { ColumnMeta } from "../lib/types";

const COLS: ColumnMeta[] = [
  { name: "id", data_type: "INTEGER", nullable: false },
  { name: "note", data_type: "TEXT", nullable: true },
  { name: "amount", data_type: "REAL", nullable: false },
];

describe("Constant-Memory Streaming Exporter & OWASP CSV Formula Guard", () => {
  it("neutralizes all OWASP CSV injection prefixes (=, +, -, @, tab, CR, leading-space formulas)", () => {
    expect(neutralizeCsvFormula("=SUM(A1:A2)")).toBe("'=SUM(A1:A2)");
    expect(neutralizeCsvFormula("+cmd|'/C calc'!A0")).toBe("'+cmd|'/C calc'!A0");
    expect(neutralizeCsvFormula("-2+3")).toBe("'-2+3");
    expect(neutralizeCsvFormula("@HYPERLINK(\"http://evil\")")).toBe("'@HYPERLINK(\"http://evil\")");
    expect(neutralizeCsvFormula("\t=1+1")).toBe("'\t=1+1");
    expect(neutralizeCsvFormula("\r=1+1")).toBe("'\r=1+1");
    expect(neutralizeCsvFormula("  =1+1")).toBe("'  =1+1");
    expect(neutralizeCsvFormula("Regular text")).toBe("Regular text");
  });

  it("exports RFC 4180 CSV with OWASP formula neutralization enabled", () => {
    const { chunks, stats } = exportRowsToPayload(
      COLS,
      [
        [1, '=HYPERLINK("https://evil.test","click")', 19.95],
        [2, 'Comma, "quoted" cell', -4.5],
        [3, null, 0],
      ],
      {
        format: "csv",
        tableName: "orders",
        batchSize: 500,
        neutralizeFormulas: true,
        includeHeader: true,
      }
    );

    expect(stats.rows_exported).toBe(3);
    const lines = chunks.join("").trim().split("\n");
    expect(lines[0]).toBe("id,note,amount");
    expect(lines[1]).toBe('1,"\'=HYPERLINK(""https://evil.test"",""click"")",19.95');
    expect(lines[2]).toBe('2,"Comma, ""quoted"" cell",-4.5');
    expect(lines[3]).toBe("3,,0");
  });

  it("exports valid NDJSON lines and batched SQL INSERT statements", () => {
    const rows = [
      [1, "O'Reilly", 25],
      [2, "Plain", 50],
      [3, null, 0],
    ];

    const ndjson = exportRowsToPayload(COLS, rows, {
      format: "ndjson",
      tableName: "orders",
      batchSize: 2,
      neutralizeFormulas: true,
      includeHeader: true,
    });
    const parsed = ndjson.chunks
      .join("")
      .trim()
      .split("\n")
      .map((line) => JSON.parse(line) as Record<string, unknown>);
    expect(parsed).toHaveLength(3);
    expect(parsed[0].note).toBe("O'Reilly");
    expect(parsed[2].note).toBeNull();

    const sqlOut = exportRowsToPayload(COLS, rows, {
      format: "sql",
      tableName: "invoices",
      batchSize: 2,
      neutralizeFormulas: true,
      includeHeader: true,
    });
    const sqlText = sqlOut.chunks.join("");
    expect(sqlText.match(/INSERT INTO "invoices"/g)).toHaveLength(2);
    expect(sqlText).toContain("'O''Reilly'");
    expect(sqlText).toContain("(3, NULL, 0)");
  });

  it("benchmarks 50,000-row streaming export cleanly", () => {
    const stats = benchmarkStreamingExport(50_000, "csv");
    expect(stats.rows_exported).toBe(50_000);
    expect(stats.bytes_written).toBeGreaterThan(1_500_000);
  });
});
