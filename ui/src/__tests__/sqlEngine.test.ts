import { describe, expect, it } from "vitest";
import { LocalBrowserEngine, formatSql, lintSql, tokenizeSql } from "../lib/sqlEngine";

describe("SQL Lexer, Formatter, Safety Linter, and Local Engine", () => {
  it("tokenizes SQL keywords, identifiers, strings, numbers, operators, and comments with exact spans", () => {
    const sql =
      "-- top orders\nSELECT o.id, 'O''Reilly' AS vendor, 42.5 FROM \"orders\" o WHERE o.total_cents >= 5000 /* ok */;";
    const tokens = tokenizeSql(sql);

    expect(tokens.some((t) => t.kind === "line_comment" && t.text === "-- top orders")).toBe(true);
    expect(tokens.some((t) => t.kind === "keyword" && t.text === "SELECT")).toBe(true);
    expect(tokens.some((t) => t.kind === "string_literal" && t.text === "'O''Reilly'")).toBe(true);
    expect(tokens.some((t) => t.kind === "number" && t.text === "42.5")).toBe(true);
    expect(tokens.some((t) => t.kind === "quoted_identifier" && t.text === '"orders"')).toBe(true);
    expect(tokens.some((t) => t.kind === "operator" && t.text === ">=")).toBe(true);
    expect(tokens.some((t) => t.kind === "block_comment" && t.text === "/* ok */")).toBe(true);

    for (const tok of tokens) {
      expect(sql.slice(tok.start, tok.end)).toBe(tok.text);
    }
  });

  it("catches destructive queries (unbounded DELETE/UPDATE, DROP TABLE, TRUNCATE, SELECT *)", () => {
    const issues = lintSql(
      "DELETE FROM orders; UPDATE users SET active = FALSE; DROP TABLE invoices; TRUNCATE orders; SELECT * FROM products LIMIT 10;"
    );
    const codes = issues.map((i) => i.code);
    expect(codes).toContain("VG001");
    expect(codes).toContain("VG002");
    expect(codes).toContain("VG003");
    expect(codes).toContain("VG004");
    expect(codes).toContain("VG006");
    expect(issues.find((i) => i.code === "VG001")?.severity).toBe("error");
    expect(issues.find((i) => i.code === "VG006")?.severity).toBe("warning");
  });

  it("does not trigger SELECT * or missing LIMIT warnings on aggregate COUNT(*) queries", () => {
    const issues = lintSql("SELECT COUNT(*) FROM orders;");
    expect(issues).toHaveLength(0);
  });

  it("pretty-formats SQL clauses and preserves string literals untouched", () => {
    const formatted = formatSql(
      "select id, email from users where active = true and country = 'select from' order by id desc limit 10;"
    );
    expect(formatted).toBe(
      "SELECT\n  id,\n  email\nFROM users\nWHERE active = TRUE\n  AND country = 'select from'\nORDER BY id DESC\nLIMIT 10;"
    );
  });

  it("executes JOIN, GROUP BY, and EXPLAIN queries on the seeded 580-row dataset", () => {
    const engine = new LocalBrowserEngine();
    const tables = engine.listTables();
    const totalRows = tables.reduce((acc, t) => acc + t.row_estimate, 0);
    expect(tables).toHaveLength(4);
    expect(totalRows).toBe(580);

    const joined = engine.executeQuery(
      "SELECT o.id, u.email, o.total_cents FROM orders o INNER JOIN users u ON o.user_id = u.id WHERE o.status = 'paid' ORDER BY o.total_cents DESC LIMIT 5;"
    );
    expect(joined.rows).toHaveLength(5);

    const grouped = engine.executeQuery(
      "SELECT status, COUNT(*) AS cnt, SUM(total_cents) AS gross FROM orders GROUP BY status ORDER BY cnt DESC;"
    );
    expect(grouped.rows).toHaveLength(5);

    const explain = engine.executeQuery("EXPLAIN SELECT id FROM users WHERE active = TRUE LIMIT 10;");
    expect(explain.command_tag).toBe("EXPLAIN");
    expect(explain.rows.length).toBeGreaterThanOrEqual(3);
  });
});
