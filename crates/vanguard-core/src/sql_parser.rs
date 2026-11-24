//! Zero-dependency SQL lexer, statement analyzer, pretty-formatter, and destructive-query safety linter.

use serde::{Deserialize, Serialize};

/// Lexical category for syntax highlighting and downstream parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    Keyword,
    Identifier,
    QuotedIdentifier,
    StringLiteral,
    Number,
    Operator,
    Punctuation,
    LineComment,
    BlockComment,
    Whitespace,
    Unknown,
}

/// A single lexical token with exact byte offsets into the source SQL string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSpan {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// High-level SQL statement classification extracted from the token stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatementKind {
    Select,
    Insert,
    Update,
    Delete,
    CreateTable,
    Drop,
    Truncate,
    Alter,
    Explain,
    Transaction,
    Other,
}

/// Structural summary of a single SQL statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedStatement {
    pub index: usize,
    pub kind: StatementKind,
    pub start: usize,
    pub end: usize,
    pub has_where: bool,
    pub has_limit: bool,
    pub has_select_star: bool,
    pub has_aggregate: bool,
    pub has_alter_drop: bool,
    pub drop_object_kind: Option<String>,
    pub tokens: Vec<TokenSpan>,
}

/// Severity level for safety linter findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    Error,
    Warning,
    Info,
}

/// A single safety linter diagnostic attached to a byte range and statement index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintIssue {
    pub code: String,
    pub severity: LintSeverity,
    pub message: String,
    pub suggestion: String,
    pub statement_index: usize,
    pub start: usize,
    pub end: usize,
}

const SQL_KEYWORDS: &[&str] = &[
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
];

const AGGREGATE_FUNCS: &[&str] = &["COUNT", "SUM", "AVG", "MIN", "MAX"];

/// Returns true if `word` is a recognized SQL keyword (case-insensitive).
#[must_use]
pub fn is_sql_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    SQL_KEYWORDS.contains(&upper.as_str())
}

/// Tokenizes an arbitrary SQL string into exact byte-offset spans without external dependencies.
#[must_use]
pub fn tokenize_sql(input: &str) -> Vec<TokenSpan> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < len {
        let b = bytes[i];

        // Whitespace
        if b.is_ascii_whitespace() {
            let start = i;
            while i < len && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            tokens.push(TokenSpan {
                kind: TokenKind::Whitespace,
                start,
                end: i,
                text: input[start..i].to_string(),
            });
            continue;
        }

        // Line comment: -- ...
        if b == b'-' && i + 1 < len && bytes[i + 1] == b'-' {
            let start = i;
            i += 2;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            tokens.push(TokenSpan {
                kind: TokenKind::LineComment,
                start,
                end: i,
                text: input[start..i].to_string(),
            });
            continue;
        }

        // Block comment: /* ... */
        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            let mut depth = 1usize;
            while i < len && depth > 0 {
                if bytes[i] == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && i + 1 < len && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            tokens.push(TokenSpan {
                kind: TokenKind::BlockComment,
                start,
                end: i,
                text: input[start..i].to_string(),
            });
            continue;
        }

        // Single-quoted string literal with SQL '' escaping
        if b == b'\'' {
            let start = i;
            i += 1;
            while i < len {
                if bytes[i] == b'\'' {
                    i += 1;
                    if i < len && bytes[i] == b'\'' {
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            tokens.push(TokenSpan {
                kind: TokenKind::StringLiteral,
                start,
                end: i,
                text: input[start..i].to_string(),
            });
            continue;
        }

        // Double-quoted or backtick-quoted identifier
        if b == b'"' || b == b'`' {
            let quote = b;
            let start = i;
            i += 1;
            while i < len {
                if bytes[i] == quote {
                    i += 1;
                    if i < len && bytes[i] == quote {
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            tokens.push(TokenSpan {
                kind: TokenKind::QuotedIdentifier,
                start,
                end: i,
                text: input[start..i].to_string(),
            });
            continue;
        }

        // Numeric literal
        if b.is_ascii_digit() {
            let start = i;
            if b == b'0' && i + 1 < len && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X') {
                i += 2;
                while i < len && bytes[i].is_ascii_hexdigit() {
                    i += 1;
                }
            } else {
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < len && bytes[i] == b'.' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
                    i += 1;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < len && (bytes[i] == b'e' || bytes[i] == b'E') {
                    let exp_start = i;
                    i += 1;
                    if i < len && (bytes[i] == b'+' || bytes[i] == b'-') {
                        i += 1;
                    }
                    if i < len && bytes[i].is_ascii_digit() {
                        while i < len && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                    } else {
                        i = exp_start;
                    }
                }
            }
            tokens.push(TokenSpan {
                kind: TokenKind::Number,
                start,
                end: i,
                text: input[start..i].to_string(),
            });
            continue;
        }

        // Identifier or keyword
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            i += 1;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'$') {
                i += 1;
            }
            let word = &input[start..i];
            let kind = if is_sql_keyword(word) {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            tokens.push(TokenSpan {
                kind,
                start,
                end: i,
                text: word.to_string(),
            });
            continue;
        }

        // Multi-character operators
        if i + 2 < len && &input[i..i + 3] == "->>" {
            tokens.push(TokenSpan {
                kind: TokenKind::Operator,
                start: i,
                end: i + 3,
                text: "->>".to_string(),
            });
            i += 3;
            continue;
        }
        if i + 1 < len {
            let two = &input[i..i + 2];
            if matches!(two, "!=" | "<>" | "<=" | ">=" | "||" | "::" | "->") {
                tokens.push(TokenSpan {
                    kind: TokenKind::Operator,
                    start: i,
                    end: i + 2,
                    text: two.to_string(),
                });
                i += 2;
                continue;
            }
        }

        // Single-character operators
        if matches!(b, b'=' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'!') {
            tokens.push(TokenSpan {
                kind: TokenKind::Operator,
                start: i,
                end: i + 1,
                text: input[i..i + 1].to_string(),
            });
            i += 1;
            continue;
        }

        // Punctuation
        if matches!(b, b',' | b';' | b'(' | b')' | b'.') {
            tokens.push(TokenSpan {
                kind: TokenKind::Punctuation,
                start: i,
                end: i + 1,
                text: input[i..i + 1].to_string(),
            });
            i += 1;
            continue;
        }

        // UTF-8 safe fallback for any unknown character
        let ch = input[i..].chars().next().unwrap_or(' ');
        let char_len = ch.len_utf8();
        tokens.push(TokenSpan {
            kind: TokenKind::Unknown,
            start: i,
            end: i + char_len,
            text: input[i..i + char_len].to_string(),
        });
        i += char_len;
    }

    tokens
}

fn is_trivia(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
    )
}

/// Parses a SQL script into individual analyzed statements split on top-level semicolons.
#[must_use]
pub fn parse_statements(sql: &str) -> Vec<ParsedStatement> {
    let all_tokens = tokenize_sql(sql);
    let mut raw_groups: Vec<Vec<TokenSpan>> = Vec::new();
    let mut current: Vec<TokenSpan> = Vec::new();
    let mut paren_depth: usize = 0;

    for token in all_tokens {
        if token.kind == TokenKind::Punctuation {
            if token.text == "(" {
                paren_depth += 1;
            } else if token.text == ")" {
                paren_depth = paren_depth.saturating_sub(1);
            } else if token.text == ";" && paren_depth == 0 {
                if current.iter().any(|t| !is_trivia(t.kind)) {
                    raw_groups.push(current);
                }
                current = Vec::new();
                continue;
            }
        }
        current.push(token);
    }

    if current.iter().any(|t| !is_trivia(t.kind)) {
        raw_groups.push(current);
    }

    raw_groups
        .into_iter()
        .enumerate()
        .map(|(index, tokens)| analyze_statement(index, tokens))
        .collect()
}

fn analyze_statement(index: usize, tokens: Vec<TokenSpan>) -> ParsedStatement {
    let sig: Vec<&TokenSpan> = tokens.iter().filter(|t| !is_trivia(t.kind)).collect();
    let start = sig.first().map_or(0, |t| t.start);
    let end = sig.last().map_or(start, |t| t.end);

    let first_upper = sig
        .first()
        .map(|t| t.text.to_ascii_uppercase())
        .unwrap_or_default();
    let second_upper = sig
        .get(1)
        .map(|t| t.text.to_ascii_uppercase())
        .unwrap_or_default();

    let kind = match first_upper.as_str() {
        "SELECT" | "WITH" => StatementKind::Select,
        "INSERT" => StatementKind::Insert,
        "UPDATE" => StatementKind::Update,
        "DELETE" => StatementKind::Delete,
        "CREATE" if second_upper == "TABLE" => StatementKind::CreateTable,
        "DROP" => StatementKind::Drop,
        "TRUNCATE" => StatementKind::Truncate,
        "ALTER" => StatementKind::Alter,
        "EXPLAIN" => StatementKind::Explain,
        "BEGIN" | "COMMIT" | "ROLLBACK" => StatementKind::Transaction,
        _ => StatementKind::Other,
    };

    let drop_object_kind = if kind == StatementKind::Drop {
        sig.get(1).map(|t| t.text.to_ascii_uppercase())
    } else {
        None
    };

    let mut depth: usize = 0;
    let mut has_where = false;
    let mut has_limit = false;
    let mut has_select_star = false;
    let mut has_aggregate = false;
    let mut has_alter_drop = false;
    let mut in_select_projection = false;

    for (idx, tok) in sig.iter().enumerate() {
        let upper = tok.text.to_ascii_uppercase();

        if tok.kind == TokenKind::Punctuation {
            if tok.text == "(" {
                depth += 1;
                continue;
            }
            if tok.text == ")" {
                depth = depth.saturating_sub(1);
                continue;
            }
        }

        if depth == 0 && tok.kind == TokenKind::Keyword {
            match upper.as_str() {
                "SELECT" => in_select_projection = true,
                "FROM" | "WHERE" | "GROUP" | "ORDER" | "LIMIT" | "HAVING" => {
                    in_select_projection = false;
                }
                _ => {}
            }
            if upper == "WHERE" {
                has_where = true;
            }
            if upper == "LIMIT" {
                has_limit = true;
            }
            if kind == StatementKind::Alter && idx > 0 && upper == "DROP" {
                has_alter_drop = true;
            }
        }

        if in_select_projection
            && tok.kind == TokenKind::Keyword
            && AGGREGATE_FUNCS.contains(&upper.as_str())
        {
            if let Some(next) = sig.get(idx + 1) {
                if next.kind == TokenKind::Punctuation && next.text == "(" {
                    has_aggregate = true;
                }
            }
        }

        // Detect SELECT * or SELECT table.* at projection depth 0 (never triggers on COUNT(*))
        if in_select_projection && depth == 0 && tok.kind == TokenKind::Operator && tok.text == "*"
        {
            let prev_is_expr_operand = idx > 0
                && matches!(
                    sig[idx - 1].kind,
                    TokenKind::Number | TokenKind::StringLiteral
                );
            if !prev_is_expr_operand {
                has_select_star = true;
            }
        }
    }

    ParsedStatement {
        index,
        kind,
        start,
        end,
        has_where,
        has_limit,
        has_select_star,
        has_aggregate,
        has_alter_drop,
        drop_object_kind,
        tokens,
    }
}

/// Runs destructive-query and best-practice safety checks across all SQL statements.
#[must_use]
pub fn lint_sql(sql: &str) -> Vec<LintIssue> {
    let statements = parse_statements(sql);
    let mut issues = Vec::new();

    for stmt in &statements {
        match stmt.kind {
            StatementKind::Delete if !stmt.has_where => {
                issues.push(LintIssue {
                    code: "VG001".to_string(),
                    severity: LintSeverity::Error,
                    message: "Unbounded DELETE without a WHERE clause will remove every row."
                        .to_string(),
                    suggestion: "Add an explicit WHERE predicate or wrap in a transaction."
                        .to_string(),
                    statement_index: stmt.index,
                    start: stmt.start,
                    end: stmt.end,
                });
            }
            StatementKind::Update if !stmt.has_where => {
                issues.push(LintIssue {
                    code: "VG002".to_string(),
                    severity: LintSeverity::Error,
                    message: "Unbounded UPDATE without a WHERE clause will modify every row."
                        .to_string(),
                    suggestion: "Add a WHERE clause targeting the intended primary keys."
                        .to_string(),
                    statement_index: stmt.index,
                    start: stmt.start,
                    end: stmt.end,
                });
            }
            StatementKind::Drop => {
                let target = stmt
                    .drop_object_kind
                    .as_deref()
                    .unwrap_or("OBJECT");
                issues.push(LintIssue {
                    code: "VG003".to_string(),
                    severity: LintSeverity::Error,
                    message: format!(
                        "Destructive DROP {target} statement detected; schema and data cannot be recovered."
                    ),
                    suggestion: "Verify target environment and take a streaming backup before executing."
                        .to_string(),
                    statement_index: stmt.index,
                    start: stmt.start,
                    end: stmt.end,
                });
            }
            StatementKind::Truncate => {
                issues.push(LintIssue {
                    code: "VG004".to_string(),
                    severity: LintSeverity::Error,
                    message: "Destructive TRUNCATE statement will immediately purge all rows."
                        .to_string(),
                    suggestion: "Confirm table name and export a snapshot before truncating."
                        .to_string(),
                    statement_index: stmt.index,
                    start: stmt.start,
                    end: stmt.end,
                });
            }
            StatementKind::Alter if stmt.has_alter_drop => {
                issues.push(LintIssue {
                    code: "VG005".to_string(),
                    severity: LintSeverity::Warning,
                    message: "ALTER statement contains a DROP operation that may permanently lose data."
                        .to_string(),
                    suggestion: "Verify dependent views, indexes, and application queries first."
                        .to_string(),
                    statement_index: stmt.index,
                    start: stmt.start,
                    end: stmt.end,
                });
            }
            StatementKind::Select => {
                if stmt.has_select_star {
                    issues.push(LintIssue {
                        code: "VG006".to_string(),
                        severity: LintSeverity::Warning,
                        message: "SELECT * retrieves all columns and can degrade wire throughput."
                            .to_string(),
                        suggestion: "Project explicit column names for predictable schemas and faster transfers."
                            .to_string(),
                        statement_index: stmt.index,
                        start: stmt.start,
                        end: stmt.end,
                    });
                }
                if !stmt.has_limit && !stmt.has_aggregate {
                    issues.push(LintIssue {
                        code: "VG007".to_string(),
                        severity: LintSeverity::Info,
                        message: "SELECT query has no LIMIT clause and may return an unbounded result set."
                            .to_string(),
                        suggestion: "Add LIMIT <n> for interactive exploration or use Streaming Export for full dumps."
                            .to_string(),
                        statement_index: stmt.index,
                        start: stmt.start,
                        end: stmt.end,
                    });
                }
            }
            _ => {}
        }
    }

    issues
}

/// Pretty-formats SQL with uppercase keywords, structured clause line breaks, and clean indentation.
#[must_use]
pub fn format_sql(sql: &str) -> String {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let tokens: Vec<TokenSpan> = tokenize_sql(trimmed)
        .into_iter()
        .filter(|t| t.kind != TokenKind::Whitespace)
        .collect();

    if tokens.is_empty() {
        return String::new();
    }

    let mut out = String::with_capacity(trimmed.len() + 32);
    let mut paren_depth: usize = 0;
    let mut in_select_clause = false;
    let mut in_where_clause = false;
    let mut i = 0;

    while i < tokens.len() {
        let tok = &tokens[i];
        let upper = tok.text.to_ascii_uppercase();

        // Preserve comments on their own line cleanly
        if matches!(tok.kind, TokenKind::LineComment | TokenKind::BlockComment) {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&tok.text);
            out.push('\n');
            i += 1;
            continue;
        }

        // Semicolon ends statement
        if tok.kind == TokenKind::Punctuation && tok.text == ";" {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push(';');
            if i + 1 < tokens.len() {
                out.push_str("\n\n");
            }
            in_select_clause = false;
            in_where_clause = false;
            paren_depth = 0;
            i += 1;
            continue;
        }

        // Handle two-word clauses at top level (ORDER BY, GROUP BY, INNER JOIN, LEFT JOIN, etc.)
        if paren_depth == 0 && tok.kind == TokenKind::Keyword {
            let next_upper = tokens
                .get(i + 1)
                .filter(|t| t.kind == TokenKind::Keyword)
                .map(|t| t.text.to_ascii_uppercase());

            let two_word = match (upper.as_str(), next_upper.as_deref()) {
                ("ORDER", Some("BY")) => Some("ORDER BY"),
                ("GROUP", Some("BY")) => Some("GROUP BY"),
                ("INNER", Some("JOIN")) => Some("INNER JOIN"),
                ("LEFT", Some("JOIN")) => Some("LEFT JOIN"),
                ("RIGHT", Some("JOIN")) => Some("RIGHT JOIN"),
                ("FULL", Some("JOIN")) => Some("FULL JOIN"),
                ("CROSS", Some("JOIN")) => Some("CROSS JOIN"),
                ("DELETE", Some("FROM")) => Some("DELETE FROM"),
                ("INSERT", Some("INTO")) => Some("INSERT INTO"),
                ("UNION", Some("ALL")) => Some("UNION ALL"),
                _ => None,
            };

            if let Some(phrase) = two_word {
                if !out.is_empty() && !out.ends_with('\n') {
                    trim_trailing_spaces(&mut out);
                    out.push('\n');
                }
                out.push_str(phrase);
                in_select_clause = false;
                in_where_clause = false;
                i += 2;
                continue;
            }

            // Single-word major clause starts a new line
            let is_major_clause = matches!(
                upper.as_str(),
                "SELECT"
                    | "FROM"
                    | "WHERE"
                    | "JOIN"
                    | "HAVING"
                    | "LIMIT"
                    | "OFFSET"
                    | "SET"
                    | "VALUES"
                    | "RETURNING"
                    | "UNION"
                    | "UPDATE"
            );

            if is_major_clause {
                if !out.is_empty() && !out.ends_with('\n') {
                    trim_trailing_spaces(&mut out);
                    out.push('\n');
                }
                out.push_str(&upper);
                if upper == "SELECT" {
                    in_select_clause = true;
                    in_where_clause = false;
                    // Place projections on indented line if not immediately DISTINCT/ALL
                    let next_is_mod = tokens.get(i + 1).is_some_and(|n| {
                        n.kind == TokenKind::Keyword
                            && matches!(n.text.to_ascii_uppercase().as_str(), "DISTINCT" | "ALL")
                    });
                    if !next_is_mod {
                        out.push_str("\n  ");
                    }
                } else if upper == "WHERE" {
                    in_select_clause = false;
                    in_where_clause = true;
                } else {
                    in_select_clause = false;
                    in_where_clause = false;
                }
                i += 1;
                continue;
            }

            // Indented boolean connectors in WHERE
            if in_where_clause && matches!(upper.as_str(), "AND" | "OR") {
                trim_trailing_spaces(&mut out);
                out.push_str("\n  ");
                out.push_str(&upper);
                i += 1;
                continue;
            }
        }

        // Punctuation rules
        if tok.kind == TokenKind::Punctuation {
            match tok.text.as_str() {
                "(" => {
                    // No space before '(' if immediately following a function name or keyword like COUNT/VALUES?
                    // Keep space after SQL keywords like IN/VALUES/ON/AND/WHERE/SET, no space after function/identifier
                    let prev_kind = i.checked_sub(1).map(|p| &tokens[p]);
                    let need_space = prev_kind.is_some_and(|p| {
                        p.kind == TokenKind::Keyword
                            && !AGGREGATE_FUNCS.contains(&p.text.to_ascii_uppercase().as_str())
                            && !matches!(
                                p.text.to_ascii_uppercase().as_str(),
                                "COALESCE" | "CAST"
                            )
                    });
                    if need_space && !out.is_empty() && !out.ends_with([' ', '\n', '(']) {
                        out.push(' ');
                    }
                    out.push('(');
                    paren_depth += 1;
                }
                ")" => {
                    trim_trailing_spaces(&mut out);
                    out.push(')');
                    paren_depth = paren_depth.saturating_sub(1);
                }
                "," => {
                    trim_trailing_spaces(&mut out);
                    out.push(',');
                    if in_select_clause && paren_depth == 0 {
                        out.push_str("\n  ");
                    } else {
                        out.push(' ');
                    }
                }
                "." => {
                    trim_trailing_spaces(&mut out);
                    out.push('.');
                }
                _ => out.push_str(&tok.text),
            }
            i += 1;
            continue;
        }

        // Spacing before regular tokens
        let prev = i.checked_sub(1).map(|p| &tokens[p]);
        let suppress_space = out.is_empty()
            || out.ends_with([' ', '\n', '(', '.'])
            || (tok.kind == TokenKind::Operator && tok.text == "::")
            || prev.is_some_and(|p| p.kind == TokenKind::Operator && p.text == "::");

        if !suppress_space {
            out.push(' ');
        }

        if tok.kind == TokenKind::Keyword {
            out.push_str(&upper);
            if in_select_clause && matches!(upper.as_str(), "DISTINCT" | "ALL") {
                out.push_str("\n  ");
            }
        } else {
            out.push_str(&tok.text);
        }

        i += 1;
    }

    trim_trailing_spaces(&mut out);
    out
}

fn trim_trailing_spaces(s: &mut String) {
    while s.ends_with(' ') {
        s.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_keywords_literals_comments_and_operators() {
        let sql = "-- fetch active\nSELECT u.id, 'O''Reilly' AS author, 42.5 FROM \"users\" u WHERE u.score >= 100 /* ok */;";
        let tokens = tokenize_sql(sql);

        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::LineComment && t.text == "-- fetch active"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Keyword && t.text == "SELECT"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::StringLiteral && t.text == "'O''Reilly'"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Number && t.text == "42.5"));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::QuotedIdentifier && t.text == "\"users\""));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::Operator && t.text == ">="));
        assert!(tokens
            .iter()
            .any(|t| t.kind == TokenKind::BlockComment && t.text == "/* ok */"));

        // Every span must map accurately back to source slice
        for tok in &tokens {
            assert_eq!(&sql[tok.start..tok.end], tok.text);
        }
    }

    #[test]
    fn lint_catches_destructive_delete_and_update_without_where() {
        let issues = lint_sql("DELETE FROM orders; UPDATE users SET active = false;");
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].code, "VG001");
        assert_eq!(issues[0].severity, LintSeverity::Error);
        assert_eq!(issues[0].statement_index, 0);

        assert_eq!(issues[1].code, "VG002");
        assert_eq!(issues[1].severity, LintSeverity::Error);
        assert_eq!(issues[1].statement_index, 1);
    }

    #[test]
    fn lint_allows_scoped_delete_and_update_with_where() {
        let issues = lint_sql(
            "DELETE FROM orders WHERE id = 42; UPDATE users SET tier = 'pro' WHERE id = 7;",
        );
        assert!(issues.is_empty(), "expected no issues, got {issues:?}");
    }

    #[test]
    fn lint_catches_drop_truncate_alter_drop_and_select_star() {
        let sql = "DROP TABLE invoices; TRUNCATE TABLE orders; ALTER TABLE users DROP COLUMN phone; SELECT * FROM products LIMIT 10;";
        let issues = lint_sql(sql);

        let codes: Vec<&str> = issues.iter().map(|i| i.code.as_str()).collect();
        assert!(codes.contains(&"VG003"));
        assert!(codes.contains(&"VG004"));
        assert!(codes.contains(&"VG005"));
        assert!(codes.contains(&"VG006"));
        assert!(!codes.contains(&"VG007")); // LIMIT is present
    }

    #[test]
    fn lint_does_not_false_positive_select_star_on_count_star() {
        let issues = lint_sql("SELECT COUNT(*) FROM orders;");
        assert!(
            issues.is_empty(),
            "COUNT(*) should not trigger VG006 or VG007: {issues:?}"
        );
    }

    #[test]
    fn format_sql_formats_clauses_and_preserves_strings() {
        let raw = "select id, email, status from users where active = true and country = 'select from' order by id desc limit 25;";
        let formatted = format_sql(raw);
        let expected = "\
SELECT
  id,
  email,
  status
FROM users
WHERE active = TRUE
  AND country = 'select from'
ORDER BY id DESC
LIMIT 25;";
        assert_eq!(formatted, expected);
    }
}
