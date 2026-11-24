import type {
  CellValue,
  ColumnMeta,
  ColumnSchema,
  DriverPluginManifest,
  ExportFormat,
  ExportOptions,
  ExportStats,
  LintIssue,
  PageSlice,
  QueryResult,
  TableSchema,
  TokenKind,
  TokenSpan,
} from "./types";

const SQL_KEYWORDS = new Set([
  "SELECT",
  "FROM",
  "WHERE",
  "JOIN",
  "INNER",
  "LEFT",
  "RIGHT",
  "FULL",
  "OUTER",
  "CROSS",
  "ON",
  "AS",
  "CREATE",
  "ALTER",
  "DROP",
  "DELETE",
  "UPDATE",
  "INSERT",
  "INTO",
  "VALUES",
  "SET",
  "TABLE",
  "DATABASE",
  "SCHEMA",
  "INDEX",
  "VIEW",
  "COLUMN",
  "TRUNCATE",
  "EXPLAIN",
  "ANALYZE",
  "ORDER",
  "GROUP",
  "BY",
  "HAVING",
  "LIMIT",
  "OFFSET",
  "DISTINCT",
  "ALL",
  "AND",
  "OR",
  "NOT",
  "IN",
  "IS",
  "NULL",
  "TRUE",
  "FALSE",
  "LIKE",
  "ILIKE",
  "BETWEEN",
  "CASE",
  "WHEN",
  "THEN",
  "ELSE",
  "END",
  "ASC",
  "DESC",
  "UNION",
  "INTERSECT",
  "EXCEPT",
  "WITH",
  "PRIMARY",
  "KEY",
  "FOREIGN",
  "REFERENCES",
  "UNIQUE",
  "CHECK",
  "DEFAULT",
  "IF",
  "EXISTS",
  "BEGIN",
  "COMMIT",
  "ROLLBACK",
  "RETURNING",
  "COUNT",
  "SUM",
  "AVG",
  "MIN",
  "MAX",
  "COALESCE",
  "CAST",
  "INTEGER",
  "INT",
  "BIGINT",
  "SMALLINT",
  "TEXT",
  "VARCHAR",
  "CHAR",
  "REAL",
  "FLOAT",
  "DOUBLE",
  "NUMERIC",
  "DECIMAL",
  "BOOLEAN",
  "BOOL",
  "TIMESTAMP",
  "DATE",
  "JSON",
  "JSONB",
]);

const AGGREGATE_FUNCS = new Set(["COUNT", "SUM", "AVG", "MIN", "MAX"]);

export function tokenizeSql(input: string): TokenSpan[] {
  const tokens: TokenSpan[] = [];
  const len = input.length;
  let i = 0;

  while (i < len) {
    const ch = input[i];

    if (/\s/.test(ch)) {
      const start = i;
      while (i < len && /\s/.test(input[i])) i += 1;
      tokens.push({ kind: "whitespace", start, end: i, text: input.slice(start, i) });
      continue;
    }

    if (ch === "-" && input[i + 1] === "-") {
      const start = i;
      i += 2;
      while (i < len && input[i] !== "\n") i += 1;
      tokens.push({ kind: "line_comment", start, end: i, text: input.slice(start, i) });
      continue;
    }

    if (ch === "/" && input[i + 1] === "*") {
      const start = i;
      i += 2;
      let depth = 1;
      while (i < len && depth > 0) {
        if (input[i] === "/" && input[i + 1] === "*") {
          depth += 1;
          i += 2;
        } else if (input[i] === "*" && input[i + 1] === "/") {
          depth -= 1;
          i += 2;
        } else {
          i += 1;
        }
      }
      tokens.push({ kind: "block_comment", start, end: i, text: input.slice(start, i) });
      continue;
    }

    if (ch === "'") {
      const start = i;
      i += 1;
      while (i < len) {
        if (input[i] === "'") {
          i += 1;
          if (i < len && input[i] === "'") {
            i += 1;
          } else {
            break;
          }
        } else {
          i += 1;
        }
      }
      tokens.push({ kind: "string_literal", start, end: i, text: input.slice(start, i) });
      continue;
    }

    if (ch === '"' || ch === "`") {
      const quote = ch;
      const start = i;
      i += 1;
      while (i < len) {
        if (input[i] === quote) {
          i += 1;
          if (i < len && input[i] === quote) {
            i += 1;
          } else {
            break;
          }
        } else {
          i += 1;
        }
      }
      tokens.push({ kind: "quoted_identifier", start, end: i, text: input.slice(start, i) });
      continue;
    }

    if (/[0-9]/.test(ch)) {
      const start = i;
      while (i < len && /[0-9]/.test(input[i])) i += 1;
      if (input[i] === "." && /[0-9]/.test(input[i + 1] ?? "")) {
        i += 1;
        while (i < len && /[0-9]/.test(input[i])) i += 1;
      }
      tokens.push({ kind: "number", start, end: i, text: input.slice(start, i) });
      continue;
    }

    if (/[A-Za-z_]/.test(ch)) {
      const start = i;
      i += 1;
      while (i < len && /[A-Za-z0-9_$]/.test(input[i])) i += 1;
      const text = input.slice(start, i);
      const kind: TokenKind = SQL_KEYWORDS.has(text.toUpperCase()) ? "keyword" : "identifier";
      tokens.push({ kind, start, end: i, text });
      continue;
    }

    const three = input.slice(i, i + 3);
    if (three === "->>") {
      tokens.push({ kind: "operator", start: i, end: i + 3, text: three });
      i += 3;
      continue;
    }

    const two = input.slice(i, i + 2);
    if (["!=", "<>", "<=", ">=", "||", "::", "->"].includes(two)) {
      tokens.push({ kind: "operator", start: i, end: i + 2, text: two });
      i += 2;
      continue;
    }

    if ("=<>+-*/%!".includes(ch)) {
      tokens.push({ kind: "operator", start: i, end: i + 1, text: ch });
      i += 1;
      continue;
    }

    if (",;().".includes(ch)) {
      tokens.push({ kind: "punctuation", start: i, end: i + 1, text: ch });
      i += 1;
      continue;
    }

    tokens.push({ kind: "unknown", start: i, end: i + 1, text: ch });
    i += 1;
  }

  return tokens;
}

function isTrivia(kind: TokenKind): boolean {
  return kind === "whitespace" || kind === "line_comment" || kind === "block_comment";
}

export function lintSql(sql: string): LintIssue[] {
  const all = tokenizeSql(sql);
  const groups: TokenSpan[][] = [];
  let current: TokenSpan[] = [];
  let depth = 0;

  for (const tok of all) {
    if (tok.kind === "punctuation") {
      if (tok.text === "(") depth += 1;
      else if (tok.text === ")") depth = Math.max(0, depth - 1);
      else if (tok.text === ";" && depth === 0) {
        if (current.some((t) => !isTrivia(t.kind))) groups.push(current);
        current = [];
        continue;
      }
    }
    current.push(tok);
  }
  if (current.some((t) => !isTrivia(t.kind))) groups.push(current);

  const issues: LintIssue[] = [];

  groups.forEach((group, stmtIndex) => {
    const sig = group.filter((t) => !isTrivia(t.kind));
    if (sig.length === 0) return;

    const start = sig[0].start;
    const end = sig[sig.length - 1].end;
    const first = sig[0].text.toUpperCase();
    const second = sig[1]?.text.toUpperCase() ?? "";

    let pDepth = 0;
    let hasWhere = false;
    let hasLimit = false;
    let hasSelectStar = false;
    let hasAggregate = false;
    let hasAlterDrop = false;
    let inSelectProj = false;

    for (let idx = 0; idx < sig.length; idx += 1) {
      const tok = sig[idx];
      const upper = tok.text.toUpperCase();
      if (tok.kind === "punctuation") {
        if (tok.text === "(") {
          pDepth += 1;
          continue;
        }
        if (tok.text === ")") {
          pDepth = Math.max(0, pDepth - 1);
          continue;
        }
      }

      if (pDepth === 0 && tok.kind === "keyword") {
        if (upper === "SELECT") inSelectProj = true;
        else if (["FROM", "WHERE", "GROUP", "ORDER", "LIMIT", "HAVING"].includes(upper)) {
          inSelectProj = false;
        }
        if (upper === "WHERE") hasWhere = true;
        if (upper === "LIMIT") hasLimit = true;
        if (first === "ALTER" && idx > 0 && upper === "DROP") hasAlterDrop = true;
      }

      if (
        inSelectProj &&
        tok.kind === "keyword" &&
        AGGREGATE_FUNCS.has(upper) &&
        sig[idx + 1]?.kind === "punctuation" &&
        sig[idx + 1]?.text === "("
      ) {
        hasAggregate = true;
      }

      if (inSelectProj && pDepth === 0 && tok.kind === "operator" && tok.text === "*") {
        const prev = sig[idx - 1];
        const prevIsNumeric = prev && (prev.kind === "number" || prev.kind === "string_literal");
        if (!prevIsNumeric) hasSelectStar = true;
      }
    }

    if (first === "DELETE" && !hasWhere) {
      issues.push({
        code: "VG001",
        severity: "error",
        message: "Unbounded DELETE without a WHERE clause will remove every row.",
        suggestion: "Add an explicit WHERE predicate or wrap in a transaction.",
        statement_index: stmtIndex,
        start,
        end,
      });
    } else if (first === "UPDATE" && !hasWhere) {
      issues.push({
        code: "VG002",
        severity: "error",
        message: "Unbounded UPDATE without a WHERE clause will modify every row.",
        suggestion: "Add a WHERE clause targeting the intended primary keys.",
        statement_index: stmtIndex,
        start,
        end,
      });
    } else if (first === "DROP") {
      issues.push({
        code: "VG003",
        severity: "error",
        message: `Destructive DROP ${second || "OBJECT"} statement detected; schema and data cannot be recovered.`,
        suggestion: "Verify target environment and take a streaming backup before executing.",
        statement_index: stmtIndex,
        start,
        end,
      });
    } else if (first === "TRUNCATE") {
      issues.push({
        code: "VG004",
        severity: "error",
        message: "Destructive TRUNCATE statement will immediately purge all rows.",
        suggestion: "Confirm table name and export a snapshot before truncating.",
        statement_index: stmtIndex,
        start,
        end,
      });
    } else if (first === "ALTER" && hasAlterDrop) {
      issues.push({
        code: "VG005",
        severity: "warning",
        message: "ALTER statement contains a DROP operation that may permanently lose data.",
        suggestion: "Verify dependent views, indexes, and application queries first.",
        statement_index: stmtIndex,
        start,
        end,
      });
    } else if (first === "SELECT" || first === "WITH") {
      if (hasSelectStar) {
        issues.push({
          code: "VG006",
          severity: "warning",
          message: "SELECT * retrieves all columns and can degrade wire throughput.",
          suggestion: "Project explicit column names for predictable schemas and faster transfers.",
          statement_index: stmtIndex,
          start,
          end,
        });
      }
      if (!hasLimit && !hasAggregate) {
        issues.push({
          code: "VG007",
          severity: "info",
          message: "SELECT query has no LIMIT clause and may return an unbounded result set.",
          suggestion: "Add LIMIT <n> for interactive exploration or use Streaming Export.",
          statement_index: stmtIndex,
          start,
          end,
        });
      }
    }
  });

  return issues;
}

export function formatSql(sql: string): string {
  const trimmed = sql.trim();
  if (!trimmed) return "";

  const tokens = tokenizeSql(trimmed).filter((t) => t.kind !== "whitespace");
  if (tokens.length === 0) return "";

  let out = "";
  let parenDepth = 0;
  let inSelect = false;
  let inWhere = false;

  const trimTrailingSpaces = () => {
    out = out.replace(/ +$/, "");
  };

  for (let i = 0; i < tokens.length; i += 1) {
    const tok = tokens[i];
    const upper = tok.text.toUpperCase();

    if (tok.kind === "line_comment" || tok.kind === "block_comment") {
      if (out && !out.endsWith("\n")) out += "\n";
      out += `${tok.text}\n`;
      continue;
    }

    if (tok.kind === "punctuation" && tok.text === ";") {
      trimTrailingSpaces();
      out += ";";
      if (i + 1 < tokens.length) out += "\n\n";
      inSelect = false;
      inWhere = false;
      parenDepth = 0;
      continue;
    }

    if (parenDepth === 0 && tok.kind === "keyword") {
      const next = tokens[i + 1];
      const nextUpper = next?.kind === "keyword" ? next.text.toUpperCase() : "";
      const pair = `${upper} ${nextUpper}`;
      const twoWordClauses = new Set([
        "ORDER BY",
        "GROUP BY",
        "INNER JOIN",
        "LEFT JOIN",
        "RIGHT JOIN",
        "FULL JOIN",
        "CROSS JOIN",
        "DELETE FROM",
        "INSERT INTO",
        "UNION ALL",
      ]);

      if (twoWordClauses.has(pair)) {
        if (out && !out.endsWith("\n")) {
          trimTrailingSpaces();
          out += "\n";
        }
        out += pair;
        inSelect = false;
        inWhere = false;
        i += 1;
        continue;
      }

      const majorClauses = new Set([
        "SELECT",
        "FROM",
        "WHERE",
        "JOIN",
        "HAVING",
        "LIMIT",
        "OFFSET",
        "SET",
        "VALUES",
        "RETURNING",
        "UNION",
        "UPDATE",
      ]);

      if (majorClauses.has(upper)) {
        if (out && !out.endsWith("\n")) {
          trimTrailingSpaces();
          out += "\n";
        }
        out += upper;
        if (upper === "SELECT") {
          inSelect = true;
          inWhere = false;
          const nextMod =
            next?.kind === "keyword" && ["DISTINCT", "ALL"].includes(next.text.toUpperCase());
          if (!nextMod) out += "\n  ";
        } else if (upper === "WHERE") {
          inSelect = false;
          inWhere = true;
        } else {
          inSelect = false;
          inWhere = false;
        }
        continue;
      }

      if (inWhere && (upper === "AND" || upper === "OR")) {
        trimTrailingSpaces();
        out += `\n  ${upper}`;
        continue;
      }
    }

    if (tok.kind === "punctuation") {
      if (tok.text === "(") {
        const prev = tokens[i - 1];
        const needSpace =
          prev?.kind === "keyword" &&
          !AGGREGATE_FUNCS.has(prev.text.toUpperCase()) &&
          !["COALESCE", "CAST"].includes(prev.text.toUpperCase());
        if (needSpace && out && !/[ \n(]$/.test(out)) out += " ";
        out += "(";
        parenDepth += 1;
      } else if (tok.text === ")") {
        trimTrailingSpaces();
        out += ")";
        parenDepth = Math.max(0, parenDepth - 1);
      } else if (tok.text === ",") {
        trimTrailingSpaces();
        out += inSelect && parenDepth === 0 ? ",\n  " : ", ";
      } else if (tok.text === ".") {
        trimTrailingSpaces();
        out += ".";
      } else {
        out += tok.text;
      }
      continue;
    }

    const prev = tokens[i - 1];
    const suppressSpace =
      !out ||
      /[ \n(.]$/.test(out) ||
      (tok.kind === "operator" && tok.text === "::") ||
      (prev?.kind === "operator" && prev.text === "::");

    if (!suppressSpace) out += " ";
    if (tok.kind === "keyword") {
      out += upper;
      if (inSelect && (upper === "DISTINCT" || upper === "ALL")) out += "\n  ";
    } else {
      out += tok.text;
    }
  }

  trimTrailingSpaces();
  return out;
}

/**
 * OWASP CSV Formula Injection neutralization.
 * Prefixes text cells starting with =, +, -, @, \t, or \r (even after leading spaces) with `'`.
 */
export function neutralizeCsvFormula(cell: string): string {
  const trimmedLeadingSpaces = cell.replace(/^ +/, "");
  const firstRaw = cell[0];
  const firstNonSpace = trimmedLeadingSpaces[0];
  const triggers = new Set(["=", "+", "-", "@", "\t", "\r"]);
  if ((firstRaw && triggers.has(firstRaw)) || (firstNonSpace && triggers.has(firstNonSpace))) {
    return `'${cell}`;
  }
  return cell;
}

function escapeCsvCell(raw: string): string {
  if (/[",\n\r]/.test(raw)) {
    return `"${raw.replace(/"/g, '""')}"`;
  }
  return raw;
}

function formatSqlValue(val: CellValue): string {
  if (val === null) return "NULL";
  if (typeof val === "boolean") return val ? "TRUE" : "FALSE";
  if (typeof val === "number") return Number.isFinite(val) ? String(val) : "NULL";
  return `'${val.replace(/'/g, "''")}'`;
}

/**
 * Streaming chunk generator producing CSV, NDJSON, or batched SQL INSERT output
 * in bounded chunks without forcing a giant monolithic concatenation.
 */
export function* exportRowsChunked(
  columns: ColumnMeta[],
  rows: Iterable<CellValue[]>,
  options: ExportOptions
): Generator<string, number, void> {
  let rowCount = 0;

  if (options.format === "csv") {
    if (options.includeHeader) {
      const hdr = columns
        .map((c) => escapeCsvCell(options.neutralizeFormulas ? neutralizeCsvFormula(c.name) : c.name))
        .join(",");
      yield `${hdr}\n`;
    }
    let chunk: string[] = [];
    for (const row of rows) {
      const line = row
        .map((val) => {
          if (val === null) return "";
          if (typeof val === "boolean") return val ? "true" : "false";
          if (typeof val === "number") return String(val);
          const safe = options.neutralizeFormulas ? neutralizeCsvFormula(val) : val;
          return escapeCsvCell(safe);
        })
        .join(",");
      chunk.push(line);
      rowCount += 1;
      if (chunk.length >= 250) {
        yield `${chunk.join("\n")}\n`;
        chunk = [];
      }
    }
    if (chunk.length > 0) yield `${chunk.join("\n")}\n`;
    return rowCount;
  }

  if (options.format === "ndjson") {
    let chunk: string[] = [];
    for (const row of rows) {
      const obj: Record<string, CellValue> = {};
      columns.forEach((col, idx) => {
        obj[col.name] = row[idx] ?? null;
      });
      chunk.push(JSON.stringify(obj));
      rowCount += 1;
      if (chunk.length >= 250) {
        yield `${chunk.join("\n")}\n`;
        chunk = [];
      }
    }
    if (chunk.length > 0) yield `${chunk.join("\n")}\n`;
    return rowCount;
  }

  const batchSize = Math.max(1, options.batchSize);
  const tableIdent = `"${(options.tableName.trim() || "exported_rows").replace(/"/g, '""')}"`;
  const colList = columns.map((c) => `"${c.name.replace(/"/g, '""')}"`).join(", ");
  let batch: string[] = [];

  for (const row of rows) {
    batch.push(`  (${row.map(formatSqlValue).join(", ")})`);
    rowCount += 1;
    if (batch.length >= batchSize) {
      yield `INSERT INTO ${tableIdent} (${colList}) VALUES\n${batch.join(",\n")};\n`;
      batch = [];
    }
  }
  if (batch.length > 0) {
    yield `INSERT INTO ${tableIdent} (${colList}) VALUES\n${batch.join(",\n")};\n`;
  }
  return rowCount;
}

export function exportRowsToPayload(
  columns: ColumnMeta[],
  rows: CellValue[][],
  options: ExportOptions
): { chunks: string[]; stats: ExportStats } {
  const start = performance.now();
  const chunks: string[] = [];
  let bytesWritten = 0;
  const encoder = new TextEncoder();

  for (const chunk of exportRowsChunked(columns, rows, options)) {
    chunks.push(chunk);
    bytesWritten += encoder.encode(chunk).byteLength;
  }

  const durationMs = Math.max(0.01, performance.now() - start);
  return {
    chunks,
    stats: {
      format: options.format,
      rows_exported: rows.length,
      bytes_written: bytesWritten,
      duration_ms: Number(durationMs.toFixed(2)),
      rows_per_second: Math.round(rows.length / (durationMs / 1000)),
    },
  };
}

export function benchmarkStreamingExport(rowCount: number, format: ExportFormat): ExportStats {
  const start = performance.now();
  const columns: ColumnMeta[] = [
    { name: "order_id", data_type: "INTEGER", nullable: false },
    { name: "customer_email", data_type: "TEXT", nullable: false },
    { name: "region", data_type: "TEXT", nullable: false },
    { name: "status", data_type: "TEXT", nullable: false },
    { name: "total_cents", data_type: "INTEGER", nullable: false },
  ];
  const regions = ["us-east", "us-west", "eu-central", "ap-northeast"];
  const statuses = ["paid", "shipped", "processing", "refunded"];

  function* lazyRows(): Generator<CellValue[]> {
    for (let i = 1; i <= rowCount; i += 1) {
      yield [
        i,
        `customer${i}@example.org`,
        regions[i % regions.length],
        statuses[Math.floor(i / 3) % statuses.length],
        ((i * 7919) % 48500) + 1500,
      ];
    }
  }

  let bytes = 0;
  for (const chunk of exportRowsChunked(columns, lazyRows(), {
    format,
    tableName: "orders",
    batchSize: 500,
    neutralizeFormulas: true,
    includeHeader: true,
  })) {
    bytes += chunk.length;
  }
  const durationMs = Math.max(0.01, performance.now() - start);
  return {
    format,
    rows_exported: rowCount,
    bytes_written: bytes,
    duration_ms: Number(durationMs.toFixed(2)),
    rows_per_second: Math.round(rowCount / (durationMs / 1000)),
  };
}

export function compareCellValues(a: CellValue, b: CellValue): number {
  if (a === null && b === null) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  if (typeof a === "number" && typeof b === "number") return a - b;
  if (typeof a === "boolean" && typeof b === "boolean") return Number(a) - Number(b);
  return String(a).localeCompare(String(b), undefined, { numeric: true });
}

export function sortRows(
  rows: CellValue[][],
  columnIndex: number,
  direction: "asc" | "desc"
): CellValue[][] {
  const copy = [...rows];
  copy.sort((ra, rb) => {
    const cmp = compareCellValues(ra[columnIndex] ?? null, rb[columnIndex] ?? null);
    return direction === "asc" ? cmp : -cmp;
  });
  return copy;
}

export function paginateRows<T>(rows: T[], requestedPage: number, pageSize: number): PageSlice<T> {
  const safePageSize = Math.max(1, Math.floor(pageSize));
  const totalItems = rows.length;
  const totalPages = Math.max(1, Math.ceil(totalItems / safePageSize));
  const page = Math.min(Math.max(1, Math.floor(requestedPage)), totalPages);
  const startIndex = (page - 1) * safePageSize;
  const endIndex = Math.min(startIndex + safePageSize, totalItems);
  return {
    items: rows.slice(startIndex, endIndex),
    page,
    pageSize: safePageSize,
    totalItems,
    totalPages,
    startIndex,
    endIndex,
  };
}

// --- Seeded Browser Fallback Engine (mirrors Rust vanguard-core 580-row e-commerce dataset) ---

interface InternalTable {
  schema: TableSchema;
  rows: CellValue[][];
}

function makeCol(name: string, data_type: string, pk = false): ColumnSchema {
  return { name, data_type, nullable: false, primary_key: pk, default_value: null };
}

export class LocalBrowserEngine {
  private tables = new Map<string, InternalTable>();

  constructor() {
    this.seed();
  }

  private seed(): void {
    const firstNames = [
      "Elena",
      "Marcus",
      "Priya",
      "Daniel",
      "Sofia",
      "Kenji",
      "Hannah",
      "Lucas",
      "Nadia",
      "Owen",
      "Clara",
      "Tariq",
    ];
    const lastNames = [
      "Vance",
      "Chen",
      "Patel",
      "Okafor",
      "Lindqvist",
      "Moreau",
      "Kowalski",
      "Sato",
      "Alvarez",
      "Brooks",
    ];
    const plans = ["free", "pro", "team", "enterprise"];
    const countries = ["US", "DE", "GB", "JP", "CA", "SG", "FR", "NL"];

    const users: CellValue[][] = [];
    for (let id = 1; id <= 120; id += 1) {
      const f = firstNames[(id * 7) % firstNames.length];
      const l = lastNames[(id * 3) % lastNames.length];
      const m = String((id % 10) + 1).padStart(2, "0");
      const d = String((id % 27) + 1).padStart(2, "0");
      users.push([
        id,
        `${f} ${l}`,
        `${f.toLowerCase()}.${l.toLowerCase()}.${id}@acme-commerce.io`,
        plans[(id * 5 + 1) % plans.length],
        countries[(id * 11) % countries.length],
        id % 9 !== 0,
        `2026-${m}-${d}T09:30:00Z`,
      ]);
    }
    this.tables.set("users", {
      schema: {
        name: "users",
        schema: "public",
        row_estimate: users.length,
        columns: [
          makeCol("id", "INTEGER", true),
          makeCol("name", "TEXT"),
          makeCol("email", "TEXT"),
          makeCol("plan", "TEXT"),
          makeCol("country", "TEXT"),
          makeCol("active", "BOOLEAN"),
          makeCol("created_at", "TIMESTAMP"),
        ],
      },
      rows: users,
    });

    const categories = ["hardware", "storage", "networking", "accessories", "audio", "displays"];
    const adjs = ["Precision", "Compact", "Studio", "Pro", "Ultra", "Modular", "Silent", "Carbon"];
    const nouns = [
      "NVMe Enclosure",
      "Docking Hub",
      "Mechanical Keypad",
      "4K Monitor",
      "DAC Amplifier",
      "Thunderbolt Cable",
      "UPS Battery",
      "Mesh Router",
      "Calibration Sensor",
      "Laptop Stand",
    ];

    const products: CellValue[][] = [];
    for (let id = 1; id <= 60; id += 1) {
      products.push([
        id,
        `SKU-${String(1000 + id).padStart(4, "0")}`,
        `${adjs[(id * 3) % adjs.length]} ${nouns[(id * 7) % nouns.length]} Mk${(id % 3) + 1}`,
        categories[id % categories.length],
        ((id * 3791) % 85000) + 2900,
        ((id * 17) % 240) + 4,
        id % 15 !== 0,
      ]);
    }
    this.tables.set("products", {
      schema: {
        name: "products",
        schema: "public",
        row_estimate: products.length,
        columns: [
          makeCol("id", "INTEGER", true),
          makeCol("sku", "TEXT"),
          makeCol("name", "TEXT"),
          makeCol("category", "TEXT"),
          makeCol("price_cents", "INTEGER"),
          makeCol("stock", "INTEGER"),
          makeCol("active", "BOOLEAN"),
        ],
      },
      rows: products,
    });

    const statuses = ["paid", "shipped", "delivered", "processing", "refunded"];
    const regions = ["us-east", "us-west", "eu-central", "ap-northeast"];
    const orders: CellValue[][] = [];
    for (let id = 1; id <= 240; id += 1) {
      const userId = ((id * 13) % 120) + 1;
      const productId = ((id * 7) % 60) + 1;
      const qty = (id % 5) + 1;
      const unitPrice = ((productId * 3791) % 85000) + 2900;
      const m = String((id % 10) + 1).padStart(2, "0");
      const d = String((id % 28) + 1).padStart(2, "0");
      const h = String(id % 24).padStart(2, "0");
      orders.push([
        id,
        userId,
        productId,
        qty,
        unitPrice * qty,
        statuses[(id * 3) % statuses.length],
        regions[(id * 5) % regions.length],
        `2026-${m}-${d}T${h}:15:00Z`,
      ]);
    }
    this.tables.set("orders", {
      schema: {
        name: "orders",
        schema: "public",
        row_estimate: orders.length,
        columns: [
          makeCol("id", "INTEGER", true),
          makeCol("user_id", "INTEGER"),
          makeCol("product_id", "INTEGER"),
          makeCol("quantity", "INTEGER"),
          makeCol("total_cents", "INTEGER"),
          makeCol("status", "TEXT"),
          makeCol("region", "TEXT"),
          makeCol("created_at", "TIMESTAMP"),
        ],
      },
      rows: orders,
    });

    const currencies = ["USD", "EUR", "GBP", "JPY"];
    const invoices: CellValue[][] = [];
    for (let id = 1; id <= 160; id += 1) {
      const productId = ((id * 7) % 60) + 1;
      const qty = (id % 5) + 1;
      const amount = (((productId * 3791) % 85000) + 2900) * qty;
      const m = String((id % 10) + 1).padStart(2, "0");
      const d = String((id % 28) + 1).padStart(2, "0");
      invoices.push([
        id,
        id,
        `INV-2026-${String(id).padStart(5, "0")}`,
        currencies[id % currencies.length],
        amount,
        id % 6 !== 0,
        `2026-${m}-${d}T12:00:00Z`,
      ]);
    }
    this.tables.set("invoices", {
      schema: {
        name: "invoices",
        schema: "public",
        row_estimate: invoices.length,
        columns: [
          makeCol("id", "INTEGER", true),
          makeCol("order_id", "INTEGER"),
          makeCol("invoice_number", "TEXT"),
          makeCol("currency", "TEXT"),
          makeCol("amount_cents", "INTEGER"),
          makeCol("paid", "BOOLEAN"),
          makeCol("issued_at", "TIMESTAMP"),
        ],
      },
      rows: invoices,
    });
  }

  listTables(): TableSchema[] {
    return Array.from(this.tables.values()).map((t) => ({
      ...t.schema,
      row_estimate: t.rows.length,
    }));
  }

  executeQuery(sql: string): QueryResult {
    const start = performance.now();
    const clean = sql.replace(/--[^\n]*/g, "").replace(/\/\*[\s\S]*?\*\//g, "").trim();
    const stmt = clean.replace(/;+$/, "").trim();
    if (!stmt) throw new Error("SQL statement is empty.");

    const upper = stmt.toUpperCase();
    if (upper.startsWith("EXPLAIN ")) {
      const inner = stmt.slice(8).trim();
      const fromMatch = inner.match(/\bFROM\s+([a-zA-Z_][a-zA-Z0-9_]*)/i);
      const tbl = fromMatch?.[1]?.toLowerCase() ?? "orders";
      const count = this.tables.get(tbl)?.rows.length ?? 0;
      const rows: CellValue[][] = [
        [1, "SeqScan", tbl, count, `In-memory table scan over '${tbl}'`],
      ];
      if (/\bJOIN\b/i.test(inner)) rows.push([2, "HashJoin", "joined_relation", count, "Equi-join probe"]);
      if (/\bWHERE\b/i.test(inner)) rows.push([rows.length + 1, "Filter", tbl, Math.max(1, Math.floor(count / 3)), "WHERE predicate filter"]);
      if (/\bGROUP\s+BY\b/i.test(inner)) rows.push([rows.length + 1, "HashAggregate", "groups", 5, "Grouped aggregation"]);
      if (/\bORDER\s+BY\b/i.test(inner)) rows.push([rows.length + 1, "Sort", "projection", count, "TimSort over ORDER BY keys"]);
      if (/\bLIMIT\b/i.test(inner)) rows.push([rows.length + 1, "Limit", "stream", Math.min(count, 50), "Bounded result slice"]);
      return {
        columns: [
          { name: "step", data_type: "INTEGER", nullable: false },
          { name: "operation", data_type: "TEXT", nullable: false },
          { name: "target", data_type: "TEXT", nullable: false },
          { name: "estimated_rows", data_type: "INTEGER", nullable: false },
          { name: "detail", data_type: "TEXT", nullable: false },
        ],
        rows,
        affected_rows: rows.length,
        execution_ms: Number((performance.now() - start).toFixed(3)),
        command_tag: "EXPLAIN",
      };
    }

    if (upper.startsWith("DELETE ")) {
      const m = stmt.match(/^DELETE\s+FROM\s+([a-zA-Z_][a-zA-Z0-9_]*)(?:\s+WHERE\s+(.+))?$/i);
      if (!m) throw new Error("Syntax error in DELETE statement.");
      const table = this.tables.get(m[1].toLowerCase());
      if (!table) throw new Error(`Table '${m[1]}' not found.`);
      const pred = m[2];
      const before = table.rows.length;
      if (!pred) {
        table.rows = [];
      } else {
        const colNames = table.schema.columns.map((c) => c.name.toLowerCase());
        table.rows = table.rows.filter((row) => !evalSimpleWhere(pred, row, colNames));
      }
      const deleted = before - table.rows.length;
      table.schema.row_estimate = table.rows.length;
      return statusResult(`DELETE ${deleted}`, deleted, performance.now() - start);
    }

    if (upper.startsWith("UPDATE ")) {
      const m = stmt.match(
        /^UPDATE\s+([a-zA-Z_][a-zA-Z0-9_]*)\s+SET\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*([^\s]+(?:\s+'[^']*')?)(?:\s+WHERE\s+(.+))?$/i
      );
      if (!m) throw new Error("Syntax error in UPDATE statement.");
      const table = this.tables.get(m[1].toLowerCase());
      if (!table) throw new Error(`Table '${m[1]}' not found.`);
      const colNames = table.schema.columns.map((c) => c.name.toLowerCase());
      const colIdx = colNames.indexOf(m[2].toLowerCase());
      if (colIdx < 0) throw new Error(`Unknown column '${m[2]}'.`);
      const newVal = parseScalar(m[3].trim());
      let updated = 0;
      for (const row of table.rows) {
        if (!m[4] || evalSimpleWhere(m[4], row, colNames)) {
          row[colIdx] = newVal;
          updated += 1;
        }
      }
      return statusResult(`UPDATE ${updated}`, updated, performance.now() - start);
    }

    if (!upper.startsWith("SELECT ")) {
      throw new Error("Supported statements in browser demo: SELECT, EXPLAIN, UPDATE, DELETE.");
    }

    return this.executeSelect(stmt, start);
  }

  private executeSelect(stmt: string, start: number): QueryResult {
    const selectMatch = stmt.match(
      /^SELECT\s+(DISTINCT\s+)?([\s\S]+?)\s+FROM\s+([a-zA-Z_][a-zA-Z0-9_]*)(?:\s+(?:AS\s+)?([a-zA-Z_][a-zA-Z0-9_]*))?([\s\S]*)$/i
    );
    if (!selectMatch) throw new Error("Syntax error: expected SELECT <cols> FROM <table>.");

    const isDistinct = Boolean(selectMatch[1]);
    const projRaw = selectMatch[2].trim();
    const baseName = selectMatch[3].toLowerCase();
    const maybeAlias = selectMatch[4];
    let tail = selectMatch[5].trim();

    let baseAlias = baseName;
    if (maybeAlias && !["INNER", "LEFT", "JOIN", "WHERE", "GROUP", "ORDER", "LIMIT", "OFFSET"].includes(maybeAlias.toUpperCase())) {
      baseAlias = maybeAlias.toLowerCase();
    } else if (maybeAlias) {
      tail = `${maybeAlias} ${tail}`.trim();
    }

    const baseTable = this.tables.get(baseName);
    if (!baseTable) throw new Error(`Table '${selectMatch[3]}' not found.`);

    let ctxCols: Array<{ qual: string; name: string; meta: ColumnMeta }> =
      baseTable.schema.columns.map((c) => ({
        qual: baseAlias,
        name: c.name.toLowerCase(),
        meta: { name: c.name, data_type: c.data_type, nullable: c.nullable },
      }));
    let rows: CellValue[][] = baseTable.rows.map((r) => [...r]);

    // Optional INNER / LEFT JOIN
    const joinRe =
      /^(INNER\s+JOIN|LEFT\s+(?:OUTER\s+)?JOIN|JOIN)\s+([a-zA-Z_][a-zA-Z0-9_]*)(?:\s+(?:AS\s+)?([a-zA-Z_][a-zA-Z0-9_]*))?\s+ON\s+([a-zA-Z0-9_.]+)\s*=\s*([a-zA-Z0-9_.]+)/i;
    let jm = tail.match(joinRe);
    while (jm) {
      const isLeft = jm[1].toUpperCase().startsWith("LEFT");
      const rTable = this.tables.get(jm[2].toLowerCase());
      if (!rTable) throw new Error(`Joined table '${jm[2]}' not found.`);
      const rAlias = (jm[3] || jm[2]).toLowerCase();
      const rCols = rTable.schema.columns.map((c) => ({
        qual: rAlias,
        name: c.name.toLowerCase(),
        meta: { name: c.name, data_type: c.data_type, nullable: c.nullable },
      }));
      const merged = [...ctxCols, ...rCols];
      const lIdx = resolveQualified(merged, jm[4]);
      const rIdx = resolveQualified(merged, jm[5]);
      if (lIdx < 0 || rIdx < 0) throw new Error(`Invalid JOIN ON columns: ${jm[4]} = ${jm[5]}`);

      const nextRows: CellValue[][] = [];
      for (const lr of rows) {
        let matched = false;
        for (const rr of rTable.rows) {
          const combined = [...lr, ...rr];
          if (combined[lIdx] !== null && combined[lIdx] === combined[rIdx]) {
            nextRows.push(combined);
            matched = true;
          }
        }
        if (!matched && isLeft) {
          nextRows.push([...lr, ...new Array<CellValue>(rCols.length).fill(null)]);
        }
      }
      ctxCols = merged;
      rows = nextRows;
      tail = tail.slice(jm[0].length).trim();
      jm = tail.match(joinRe);
    }

    // Parse remaining clauses: WHERE, GROUP BY, ORDER BY, LIMIT, OFFSET
    let whereClause = "";
    let groupByCol = "";
    let orderClause = "";
    let limitNum: number | null = null;
    let offsetNum = 0;

    const whereMatch = tail.match(/\bWHERE\s+([\s\S]+?)(?=\bGROUP\s+BY\b|\bORDER\s+BY\b|\bLIMIT\b|\bOFFSET\b|$)/i);
    if (whereMatch) whereClause = whereMatch[1].trim();

    const groupMatch = tail.match(/\bGROUP\s+BY\s+([a-zA-Z0-9_.]+)/i);
    if (groupMatch) groupByCol = groupMatch[1].trim();

    const orderMatch = tail.match(/\bORDER\s+BY\s+([\s\S]+?)(?=\bLIMIT\b|\bOFFSET\b|$)/i);
    if (orderMatch) orderClause = orderMatch[1].trim();

    const limitMatch = tail.match(/\bLIMIT\s+(\d+)/i);
    if (limitMatch) limitNum = Number(limitMatch[1]);

    const offsetMatch = tail.match(/\bOFFSET\s+(\d+)/i);
    if (offsetMatch) offsetNum = Number(offsetMatch[1]);

    if (whereClause) {
      const colNames = ctxCols.map((c) => `${c.qual}.${c.name}`);
      const shortNames = ctxCols.map((c) => c.name);
      rows = rows.filter((row) => evalSimpleWhere(whereClause, row, shortNames, colNames));
    }

    const projParts = splitTopLevelCommas(projRaw);
    const hasAgg = projParts.some((p) => /\b(COUNT|SUM|AVG|MIN|MAX)\s*\(/i.test(p));

    let outCols: ColumnMeta[] = [];
    let outRows: CellValue[][] = [];

    if (hasAgg || groupByCol) {
      const groups = new Map<string, CellValue[][]>();
      const gIdx = groupByCol ? resolveQualified(ctxCols, groupByCol) : -1;
      if (groupByCol && gIdx < 0) throw new Error(`Unknown GROUP BY column '${groupByCol}'.`);

      if (gIdx < 0) {
        groups.set("__all__", rows);
      } else {
        for (const r of rows) {
          const key = String(r[gIdx]);
          const bucket = groups.get(key) ?? [];
          bucket.push(r);
          groups.set(key, bucket);
        }
      }

      const specs = projParts.map((raw) => {
        const aliasMatch = raw.match(/^([\s\S]+?)\s+AS\s+([a-zA-Z_][a-zA-Z0-9_]*)$/i);
        const expr = (aliasMatch ? aliasMatch[1] : raw).trim();
        const alias = aliasMatch ? aliasMatch[2] : null;
        const aggMatch = expr.match(/^(COUNT|SUM|AVG|MIN|MAX)\s*\(\s*(\*|[a-zA-Z0-9_.]+)\s*\)$/i);
        if (aggMatch) {
          return {
            kind: "agg" as const,
            fn: aggMatch[1].toUpperCase(),
            arg: aggMatch[2],
            name: alias ?? `${aggMatch[1].toLowerCase()}_${aggMatch[2] === "*" ? "all" : aggMatch[2]}`,
          };
        }
        const idx = resolveQualified(ctxCols, expr);
        if (idx < 0) throw new Error(`Unknown column '${expr}'.`);
        return { kind: "col" as const, idx, name: alias ?? ctxCols[idx].meta.name, type: ctxCols[idx].meta.data_type };
      });

      outCols = specs.map((s) => ({
        name: s.name,
        data_type: s.kind === "col" ? s.type : s.fn === "COUNT" ? "INTEGER" : "REAL",
        nullable: true,
      }));

      for (const bucket of groups.values()) {
        outRows.push(
          specs.map((s) => {
            if (s.kind === "col") return bucket[0]?.[s.idx] ?? null;
            if (s.fn === "COUNT") {
              if (s.arg === "*") return bucket.length;
              const idx = resolveQualified(ctxCols, s.arg);
              return bucket.filter((r) => r[idx] !== null).length;
            }
            const idx = resolveQualified(ctxCols, s.arg);
            const nums = bucket.map((r) => Number(r[idx])).filter((n) => Number.isFinite(n));
            if (nums.length === 0) return null;
            if (s.fn === "SUM") return nums.reduce((a, b) => a + b, 0);
            if (s.fn === "AVG") return Number((nums.reduce((a, b) => a + b, 0) / nums.length).toFixed(2));
            if (s.fn === "MIN") return Math.min(...nums);
            return Math.max(...nums);
          })
        );
      }

      if (orderClause) {
        const [colRef, dirRaw] = orderClause.split(/\s+/);
        const desc = dirRaw?.toUpperCase() === "DESC";
        const idx = outCols.findIndex((c) => c.name.toLowerCase() === colRef.toLowerCase());
        if (idx >= 0) outRows = sortRows(outRows, idx, desc ? "desc" : "asc");
      }
    } else {
      if (orderClause) {
        const [colRef, dirRaw] = orderClause.split(/\s+/);
        const desc = dirRaw?.toUpperCase() === "DESC";
        const idx = resolveQualified(ctxCols, colRef);
        if (idx >= 0) rows = sortRows(rows, idx, desc ? "desc" : "asc");
      }

      const selectedIndices: number[] = [];
      for (const part of projParts) {
        if (part === "*") {
          ctxCols.forEach((c, idx) => {
            selectedIndices.push(idx);
            outCols.push(c.meta);
          });
        } else {
          const aliasMatch = part.match(/^([a-zA-Z0-9_.]+)\s+AS\s+([a-zA-Z_][a-zA-Z0-9_]*)$/i);
          const ref = aliasMatch ? aliasMatch[1] : part;
          const alias = aliasMatch ? aliasMatch[2] : null;
          const idx = resolveQualified(ctxCols, ref);
          if (idx < 0) throw new Error(`Unknown column '${ref}'.`);
          selectedIndices.push(idx);
          outCols.push({ ...ctxCols[idx].meta, name: alias ?? ctxCols[idx].meta.name });
        }
      }

      outRows = rows.map((r) => selectedIndices.map((idx) => r[idx]));
    }

    if (isDistinct) {
      const seen = new Set<string>();
      outRows = outRows.filter((r) => {
        const key = JSON.stringify(r);
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      });
    }

    if (offsetNum > 0) outRows = outRows.slice(offsetNum);
    if (limitNum !== null) outRows = outRows.slice(0, limitNum);

    return {
      columns: outCols,
      rows: outRows,
      affected_rows: outRows.length,
      execution_ms: Number((performance.now() - start).toFixed(3)),
      command_tag: `SELECT ${outRows.length}`,
    };
  }
}

function statusResult(tag: string, affected: number, elapsedMs: number): QueryResult {
  return {
    columns: [
      { name: "status", data_type: "TEXT", nullable: false },
      { name: "affected_rows", data_type: "INTEGER", nullable: false },
    ],
    rows: [[tag, affected]],
    affected_rows: affected,
    execution_ms: Number(elapsedMs.toFixed(3)),
    command_tag: tag,
  };
}

function resolveQualified(
  cols: Array<{ qual: string; name: string }>,
  ref: string
): number {
  const lower = ref.trim().toLowerCase();
  if (lower.includes(".")) {
    const [q, n] = lower.split(".");
    return cols.findIndex((c) => c.qual === q && c.name === n);
  }
  return cols.findIndex((c) => c.name === lower);
}

function splitTopLevelCommas(raw: string): string[] {
  const out: string[] = [];
  let cur = "";
  let depth = 0;
  for (const ch of raw) {
    if (ch === "(") depth += 1;
    else if (ch === ")") depth = Math.max(0, depth - 1);
    if (ch === "," && depth === 0) {
      out.push(cur.trim());
      cur = "";
    } else {
      cur += ch;
    }
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

function parseScalar(raw: string): CellValue {
  if (/^NULL$/i.test(raw)) return null;
  if (/^TRUE$/i.test(raw)) return true;
  if (/^FALSE$/i.test(raw)) return false;
  if (raw.startsWith("'") && raw.endsWith("'")) return raw.slice(1, -1).replace(/''/g, "'");
  const num = Number(raw);
  return Number.isNaN(num) ? raw : num;
}

function evalSimpleWhere(
  clause: string,
  row: CellValue[],
  shortNames: string[],
  qualifiedNames?: string[]
): boolean {
  const orBranches = clause.split(/\s+OR\s+/i);
  return orBranches.some((branch) => {
    const andParts = branch.split(/\s+AND\s+/i);
    return andParts.every((cond) => {
      const m = cond.trim().match(/^([a-zA-Z0-9_.]+)\s*(=|!=|<>|<=|>=|<|>|LIKE)\s*(.+)$/i);
      if (!m) return true;
      const colRef = m[1].toLowerCase();
      const op = m[2].toUpperCase();
      const rhs = parseScalar(m[3].trim());

      let idx = qualifiedNames ? qualifiedNames.indexOf(colRef) : -1;
      if (idx < 0) idx = shortNames.indexOf(colRef.includes(".") ? colRef.split(".")[1] : colRef);
      if (idx < 0) return false;
      const lhs = row[idx];
      if (lhs === null || rhs === null) return false;

      if (op === "LIKE") {
        const regexStr = String(rhs)
          .replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
          .replace(/%/g, ".*")
          .replace(/_/g, ".");
        return new RegExp(`^${regexStr}$`, "i").test(String(lhs));
      }
      const cmp = compareCellValues(lhs, rhs);
      if (op === "=") return cmp === 0;
      if (op === "!=" || op === "<>") return cmp !== 0;
      if (op === "<") return cmp < 0;
      if (op === "<=") return cmp <= 0;
      if (op === ">") return cmp > 0;
      if (op === ">=") return cmp >= 0;
      return false;
    });
  });
}

export const COMMUNITY_DRIVER_PLUGINS: DriverPluginManifest[] = [
  {
    id: "postgres-wire-v3",
    name: "PostgreSQL Wire v3 Driver",
    version: "1.0.0",
    protocol: "postgresql-v3.0",
    wire_transport: "tcp",
    default_port: 5432,
    author: "xity4uu-a11y",
    license: "MIT",
    description:
      "Pure-Rust PostgreSQL frontend/backend wire protocol v3.0 TCP client with Trust, Cleartext, and MD5 auth plus O(1)-memory streaming exports.",
    capabilities: ["query", "schema_inspect", "streaming_export", "transactions", "explain"],
    streaming_export: true,
    zero_c_deps: true,
    min_core_version: "1.0.0",
  },
  {
    id: "sqlite-wal-native",
    name: "SQLite 3 WAL Native Inspector",
    version: "1.0.0",
    protocol: "sqlite3-file-format",
    wire_transport: "file",
    default_port: null,
    author: "xity4uu-a11y",
    license: "MIT",
    description:
      "Zero-C binary SQLite 3 database header, WAL mode, schema cookie, and Page 1 B-tree inspector paired with an in-process SQL engine.",
    capabilities: ["query", "schema_inspect", "streaming_export", "wal_inspect", "explain"],
    streaming_export: true,
    zero_c_deps: true,
    min_core_version: "1.0.0",
  },
  {
    id: "mysql-protocol-v8",
    name: "MySQL Protocol v8 Connector",
    version: "1.0.0",
    protocol: "mysql-client-server-v10",
    wire_transport: "tcp",
    default_port: 3306,
    author: "xity4uu-a11y",
    license: "MIT",
    description:
      "Community manifest for MySQL 8 and MariaDB text/binary protocol connections with server-side cursor streaming export support.",
    capabilities: ["query", "schema_inspect", "streaming_export", "transactions", "explain"],
    streaming_export: true,
    zero_c_deps: true,
    min_core_version: "1.0.0",
  },
  {
    id: "duckdb-parquet-reader",
    name: "DuckDB Parquet Analytical Reader",
    version: "1.0.0",
    protocol: "duckdb-columnar-v1",
    wire_transport: "file",
    default_port: null,
    author: "xity4uu-a11y",
    license: "MIT",
    description:
      "Analytical local driver manifest targeting DuckDB database files and direct zero-copy Parquet file scanning with columnar batch exports.",
    capabilities: ["query", "schema_inspect", "streaming_export", "parquet_scan", "columnar_batch"],
    streaming_export: true,
    zero_c_deps: false,
    min_core_version: "1.0.0",
  },
  {
    id: "clickhouse-http-driver",
    name: "ClickHouse HTTP Columnar Driver",
    version: "1.0.0",
    protocol: "clickhouse-http-v2",
    wire_transport: "http",
    default_port: 8123,
    author: "xity4uu-a11y",
    license: "MIT",
    description:
      "High-throughput HTTP/NDJSON streaming driver manifest for ClickHouse analytical clusters with chunked columnar result streaming.",
    capabilities: ["query", "schema_inspect", "streaming_export", "columnar_batch", "explain"],
    streaming_export: true,
    zero_c_deps: true,
    min_core_version: "1.0.0",
  },
];
