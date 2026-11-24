export type TokenKind =
  | "keyword"
  | "identifier"
  | "quoted_identifier"
  | "string_literal"
  | "number"
  | "operator"
  | "punctuation"
  | "line_comment"
  | "block_comment"
  | "whitespace"
  | "unknown";

export interface TokenSpan {
  kind: TokenKind;
  start: number;
  end: number;
  text: string;
}

export type LintSeverity = "error" | "warning" | "info";

export interface LintIssue {
  code: string;
  severity: LintSeverity;
  message: string;
  suggestion: string;
  statement_index: number;
  start: number;
  end: number;
}

export type CellValue = string | number | boolean | null;

export interface ColumnMeta {
  name: string;
  data_type: string;
  nullable: boolean;
}

export interface QueryResult {
  columns: ColumnMeta[];
  rows: CellValue[][];
  affected_rows: number;
  execution_ms: number;
  command_tag: string;
}

export interface ColumnSchema {
  name: string;
  data_type: string;
  nullable: boolean;
  primary_key: boolean;
  default_value: string | null;
}

export interface TableSchema {
  name: string;
  schema: string;
  row_estimate: number;
  columns: ColumnSchema[];
}

export interface ConnectionProfile {
  id: string;
  name: string;
  driver: string;
  host: string | null;
  port: number | null;
  database: string;
  username: string | null;
  read_only: boolean;
}

export interface DriverPluginManifest {
  id: string;
  name: string;
  version: string;
  protocol: string;
  wire_transport: "tcp" | "file" | "http" | "in_process";
  default_port: number | null;
  author: string;
  license: string;
  description: string;
  capabilities: string[];
  streaming_export: boolean;
  zero_c_deps: boolean;
  min_core_version: string;
}

export type ExportFormat = "csv" | "ndjson" | "sql";

export interface ExportOptions {
  format: ExportFormat;
  tableName: string;
  batchSize: number;
  neutralizeFormulas: boolean;
  includeHeader: boolean;
}

export interface ExportStats {
  format: ExportFormat;
  rows_exported: number;
  bytes_written: number;
  duration_ms: number;
  rows_per_second: number;
}

export interface SortState {
  columnIndex: number;
  direction: "asc" | "desc";
}

export interface PageSlice<T> {
  items: T[];
  page: number;
  pageSize: number;
  totalItems: number;
  totalPages: number;
  startIndex: number;
  endIndex: number;
}
