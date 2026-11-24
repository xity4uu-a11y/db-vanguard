//! Pure-Rust local relational SQL engine with a seeded 580-row e-commerce dataset
//! and a binary SQLite 3 database header inspector (zero C dependencies).

use crate::drivers::DatabaseDriver;
use crate::exporter::{stream_export_rows, ExportOptions, ExportStats};
use crate::sql_parser::{tokenize_sql, TokenKind, TokenSpan};
use crate::{ColumnMeta, ColumnSchema, ConnectionProfile, DbError, QueryResult, TableSchema, Value};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::Path;
use std::time::Instant;

const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// Parsed metadata from a SQLite 3 database file's 100-byte binary header and Page 1 B-tree header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqliteHeaderInfo {
    pub page_size: u32,
    pub write_format: u8,
    pub read_format: u8,
    pub journal_mode_hint: String,
    pub reserved_bytes_per_page: u8,
    pub file_change_counter: u32,
    pub page_count: u32,
    pub freelist_trunk_page: u32,
    pub freelist_page_count: u32,
    pub schema_cookie: u32,
    pub schema_format: u32,
    pub text_encoding: String,
    pub user_version: u32,
    pub application_id: u32,
    pub sqlite_version_raw: u32,
    pub sqlite_version: String,
    pub estimated_db_bytes: u64,
    pub page1_btree_type: Option<String>,
    pub page1_cell_count: Option<u16>,
}

/// Inspects a raw byte buffer containing at least the 100-byte SQLite 3 database header.
pub fn inspect_sqlite_header(bytes: &[u8]) -> Result<SqliteHeaderInfo, DbError> {
    if bytes.len() < 100 {
        return Err(DbError::Validation(format!(
            "buffer too short ({} bytes): SQLite 3 header requires at least 100 bytes",
            bytes.len()
        )));
    }
    if &bytes[0..16] != SQLITE_MAGIC {
        return Err(DbError::Validation(
            "invalid SQLite 3 header magic; expected \"SQLite format 3\\0\"".to_string(),
        ));
    }

    let raw_page_size = u16::from_be_bytes([bytes[16], bytes[17]]);
    let page_size: u32 = if raw_page_size == 1 {
        65_536
    } else {
        u32::from(raw_page_size)
    };

    if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
        return Err(DbError::Validation(format!(
            "invalid SQLite page size: {page_size}"
        )));
    }

    let write_format = bytes[18];
    let read_format = bytes[19];
    let journal_mode_hint = match (write_format, read_format) {
        (2, 2) => "WAL".to_string(),
        (1, 1) => "Rollback Journal (Legacy)".to_string(),
        _ => format!("Custom (write={write_format}, read={read_format})"),
    };

    let reserved_bytes_per_page = bytes[20];
    let file_change_counter = u32::from_be_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let page_count = u32::from_be_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);
    let freelist_trunk_page = u32::from_be_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]);
    let freelist_page_count = u32::from_be_bytes([bytes[36], bytes[37], bytes[38], bytes[39]]);
    let schema_cookie = u32::from_be_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]);
    let schema_format = u32::from_be_bytes([bytes[44], bytes[45], bytes[46], bytes[47]]);
    let encoding_code = u32::from_be_bytes([bytes[56], bytes[57], bytes[58], bytes[59]]);
    let text_encoding = match encoding_code {
        1 => "UTF-8".to_string(),
        2 => "UTF-16le".to_string(),
        3 => "UTF-16be".to_string(),
        other => format!("Unknown ({other})"),
    };
    let user_version = u32::from_be_bytes([bytes[60], bytes[61], bytes[62], bytes[63]]);
    let application_id = u32::from_be_bytes([bytes[68], bytes[69], bytes[70], bytes[71]]);
    let sqlite_version_raw = u32::from_be_bytes([bytes[96], bytes[97], bytes[98], bytes[99]]);
    let major = sqlite_version_raw / 1_000_000;
    let minor = (sqlite_version_raw / 1_000) % 1_000;
    let patch = sqlite_version_raw % 1_000;
    let sqlite_version = format!("{major}.{minor}.{patch}");

    let estimated_db_bytes = u64::from(page_size) * u64::from(page_count);

    let (page1_btree_type, page1_cell_count) = if bytes.len() >= 108 {
        let btree_kind = match bytes[100] {
            0x02 => Some("Interior Index B-Tree (0x02)".to_string()),
            0x05 => Some("Interior Table B-Tree (0x05)".to_string()),
            0x0A => Some("Leaf Index B-Tree (0x0A)".to_string()),
            0x0D => Some("Leaf Table B-Tree (0x0D)".to_string()),
            _ => None,
        };
        let cell_count = btree_kind
            .as_ref()
            .map(|_| u16::from_be_bytes([bytes[103], bytes[104]]));
        (btree_kind, cell_count)
    } else {
        (None, None)
    };

    Ok(SqliteHeaderInfo {
        page_size,
        write_format,
        read_format,
        journal_mode_hint,
        reserved_bytes_per_page,
        file_change_counter,
        page_count,
        freelist_trunk_page,
        freelist_page_count,
        schema_cookie,
        schema_format,
        text_encoding,
        user_version,
        application_id,
        sqlite_version_raw,
        sqlite_version,
        estimated_db_bytes,
        page1_btree_type,
        page1_cell_count,
    })
}

/// Reads the header bytes of a SQLite 3 database file from disk and parses its structure.
pub fn inspect_sqlite_file(path: &Path) -> Result<SqliteHeaderInfo, DbError> {
    use std::fs::File;
    use std::io::Read;

    let mut file = File::open(path)?;
    let mut header = [0u8; 128];
    let n = file.read(&mut header)?;
    inspect_sqlite_header(&header[..n])
}

/// Constructs a valid 108-byte SQLite 3 binary header + Page 1 B-tree header (useful for fixtures/tests).
#[must_use]
pub fn build_sqlite_header_fixture(
    page_size: u32,
    page_count: u32,
    wal_mode: bool,
    schema_cells: u16,
) -> Vec<u8> {
    let mut buf = vec![0u8; 108];
    buf[0..16].copy_from_slice(SQLITE_MAGIC);
    let encoded_page_size: u16 = if page_size == 65_536 {
        1
    } else {
        page_size as u16
    };
    buf[16..18].copy_from_slice(&encoded_page_size.to_be_bytes());
    let fmt_ver = if wal_mode { 2 } else { 1 };
    buf[18] = fmt_ver;
    buf[19] = fmt_ver;
    buf[21] = 64;
    buf[22] = 32;
    buf[23] = 32;
    buf[24..28].copy_from_slice(&42u32.to_be_bytes()); // file change counter
    buf[28..32].copy_from_slice(&page_count.to_be_bytes());
    buf[40..44].copy_from_slice(&7u32.to_be_bytes()); // schema cookie
    buf[44..48].copy_from_slice(&4u32.to_be_bytes()); // schema format 4
    buf[56..60].copy_from_slice(&1u32.to_be_bytes()); // UTF-8
    buf[60..64].copy_from_slice(&1u32.to_be_bytes()); // user_version
    buf[92..96].copy_from_slice(&42u32.to_be_bytes()); // version-valid-for
    buf[96..100].copy_from_slice(&3_046_001u32.to_be_bytes()); // SQLite 3.46.1
    // Page 1 B-tree leaf table header
    buf[100] = 0x0D;
    buf[103..105].copy_from_slice(&schema_cells.to_be_bytes());
    buf
}

#[derive(Debug, Clone)]
struct TableData {
    schema: TableSchema,
    rows: Vec<Vec<Value>>,
}

/// Pure-Rust in-process relational database engine supporting SQL CRUD, joins, aggregates, and EXPLAIN.
#[derive(Debug, Clone)]
pub struct LocalEngine {
    tables: BTreeMap<String, TableData>,
    connected: bool,
    database_name: String,
}

impl Default for LocalEngine {
    fn default() -> Self {
        Self::with_ecommerce_demo()
    }
}

impl LocalEngine {
    /// Creates an empty local database engine with no pre-seeded tables.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            tables: BTreeMap::new(),
            connected: true,
            database_name: "local_memory".to_string(),
        }
    }

    /// Creates a local database engine pre-seeded with a realistic 580-row e-commerce dataset
    /// (`users`: 120, `products`: 60, `orders`: 240, `invoices`: 160).
    #[must_use]
    pub fn with_ecommerce_demo() -> Self {
        let mut engine = Self::empty();
        engine.database_name = "vanguard_ecommerce_demo".to_string();
        engine.seed_ecommerce_dataset();
        engine
    }

    fn seed_ecommerce_dataset(&mut self) {
        const FIRST_NAMES: &[&str] = &[
            "Elena", "Marcus", "Priya", "Daniel", "Sofia", "Kenji", "Hannah", "Lucas", "Nadia",
            "Owen", "Clara", "Tariq",
        ];
        const LAST_NAMES: &[&str] = &[
            "Vance", "Chen", "Patel", "Okafor", "Lindqvist", "Moreau", "Kowalski", "Sato",
            "Alvarez", "Brooks",
        ];
        const PLANS: &[&str] = &["free", "pro", "team", "enterprise"];
        const COUNTRIES: &[&str] = &["US", "DE", "GB", "JP", "CA", "SG", "FR", "NL"];

        let mut user_rows = Vec::with_capacity(120);
        for id in 1..=120i64 {
            let f = FIRST_NAMES[(id as usize * 7) % FIRST_NAMES.len()];
            let l = LAST_NAMES[(id as usize * 3) % LAST_NAMES.len()];
            let plan = PLANS[(id as usize * 5 + 1) % PLANS.len()];
            let country = COUNTRIES[(id as usize * 11) % COUNTRIES.len()];
            let active = id % 9 != 0;
            let month = (id % 10) + 1;
            let day = (id % 27) + 1;
            user_rows.push(vec![
                Value::Integer(id),
                Value::Text(format!("{f} {l}")),
                Value::Text(format!("{}.{}.{}@acme-commerce.io", f.to_ascii_lowercase(), l.to_ascii_lowercase(), id)),
                Value::Text(plan.to_string()),
                Value::Text(country.to_string()),
                Value::Bool(active),
                Value::Text(format!("2026-{month:02}-{day:02}T09:30:00Z")),
            ]);
        }

        self.insert_raw_table(
            TableSchema {
                name: "users".to_string(),
                schema: "public".to_string(),
                row_estimate: user_rows.len() as u64,
                columns: vec![
                    col_pk("id", "INTEGER"),
                    col("name", "TEXT", false),
                    col("email", "TEXT", false),
                    col("plan", "TEXT", false),
                    col("country", "TEXT", false),
                    col("active", "BOOLEAN", false),
                    col("created_at", "TIMESTAMP", false),
                ],
            },
            user_rows,
        );

        const CATEGORIES: &[&str] = &["hardware", "storage", "networking", "accessories", "audio", "displays"];
        const PRODUCT_ADJECTIVES: &[&str] = &[
            "Precision", "Compact", "Studio", "Pro", "Ultra", "Modular", "Silent", "Carbon",
        ];
        const PRODUCT_NOUNS: &[&str] = &[
            "NVMe Enclosure", "Docking Hub", "Mechanical Keypad", "4K Monitor", "DAC Amplifier",
            "Thunderbolt Cable", "UPS Battery", "Mesh Router", "Calibration Sensor", "Laptop Stand",
        ];

        let mut product_rows = Vec::with_capacity(60);
        for id in 1..=60i64 {
            let cat = CATEGORIES[(id as usize) % CATEGORIES.len()];
            let adj = PRODUCT_ADJECTIVES[(id as usize * 3) % PRODUCT_ADJECTIVES.len()];
            let noun = PRODUCT_NOUNS[(id as usize * 7) % PRODUCT_NOUNS.len()];
            let price_cents = ((id * 3_791) % 85_000) + 2_900;
            let stock = ((id * 17) % 240) + 4;
            let active = id % 15 != 0;
            product_rows.push(vec![
                Value::Integer(id),
                Value::Text(format!("SKU-{:04}", 1000 + id)),
                Value::Text(format!("{adj} {noun} Mk{}", (id % 3) + 1)),
                Value::Text(cat.to_string()),
                Value::Integer(price_cents),
                Value::Integer(stock),
                Value::Bool(active),
            ]);
        }

        self.insert_raw_table(
            TableSchema {
                name: "products".to_string(),
                schema: "public".to_string(),
                row_estimate: product_rows.len() as u64,
                columns: vec![
                    col_pk("id", "INTEGER"),
                    col("sku", "TEXT", false),
                    col("name", "TEXT", false),
                    col("category", "TEXT", false),
                    col("price_cents", "INTEGER", false),
                    col("stock", "INTEGER", false),
                    col("active", "BOOLEAN", false),
                ],
            },
            product_rows,
        );

        const ORDER_STATUSES: &[&str] = &["paid", "shipped", "delivered", "processing", "refunded"];
        const REGIONS: &[&str] = &["us-east", "us-west", "eu-central", "ap-northeast"];

        let mut order_rows = Vec::with_capacity(240);
        for id in 1..=240i64 {
            let user_id = ((id * 13) % 120) + 1;
            let product_id = ((id * 7) % 60) + 1;
            let quantity = (id % 5) + 1;
            let unit_price = ((product_id * 3_791) % 85_000) + 2_900;
            let total_cents = unit_price * quantity;
            let status = ORDER_STATUSES[(id as usize * 3) % ORDER_STATUSES.len()];
            let region = REGIONS[(id as usize * 5) % REGIONS.len()];
            let month = (id % 10) + 1;
            let day = (id % 28) + 1;
            let hour = id % 24;
            order_rows.push(vec![
                Value::Integer(id),
                Value::Integer(user_id),
                Value::Integer(product_id),
                Value::Integer(quantity),
                Value::Integer(total_cents),
                Value::Text(status.to_string()),
                Value::Text(region.to_string()),
                Value::Text(format!("2026-{month:02}-{day:02}T{hour:02}:15:00Z")),
            ]);
        }

        self.insert_raw_table(
            TableSchema {
                name: "orders".to_string(),
                schema: "public".to_string(),
                row_estimate: order_rows.len() as u64,
                columns: vec![
                    col_pk("id", "INTEGER"),
                    col("user_id", "INTEGER", false),
                    col("product_id", "INTEGER", false),
                    col("quantity", "INTEGER", false),
                    col("total_cents", "INTEGER", false),
                    col("status", "TEXT", false),
                    col("region", "TEXT", false),
                    col("created_at", "TIMESTAMP", false),
                ],
            },
            order_rows,
        );

        const CURRENCIES: &[&str] = &["USD", "EUR", "GBP", "JPY"];
        let mut invoice_rows = Vec::with_capacity(160);
        for id in 1..=160i64 {
            let order_id = id;
            let currency = CURRENCIES[(id as usize) % CURRENCIES.len()];
            let product_id = ((order_id * 7) % 60) + 1;
            let quantity = (order_id % 5) + 1;
            let amount_cents = (((product_id * 3_791) % 85_000) + 2_900) * quantity;
            let paid = id % 6 != 0;
            let month = (id % 10) + 1;
            let day = (id % 28) + 1;
            invoice_rows.push(vec![
                Value::Integer(id),
                Value::Integer(order_id),
                Value::Text(format!("INV-2026-{:05}", id)),
                Value::Text(currency.to_string()),
                Value::Integer(amount_cents),
                Value::Bool(paid),
                Value::Text(format!("2026-{month:02}-{day:02}T12:00:00Z")),
            ]);
        }

        self.insert_raw_table(
            TableSchema {
                name: "invoices".to_string(),
                schema: "public".to_string(),
                row_estimate: invoice_rows.len() as u64,
                columns: vec![
                    col_pk("id", "INTEGER"),
                    col("order_id", "INTEGER", false),
                    col("invoice_number", "TEXT", false),
                    col("currency", "TEXT", false),
                    col("amount_cents", "INTEGER", false),
                    col("paid", "BOOLEAN", false),
                    col("issued_at", "TIMESTAMP", false),
                ],
            },
            invoice_rows,
        );
    }

    fn insert_raw_table(&mut self, schema: TableSchema, rows: Vec<Vec<Value>>) {
        let key = schema.name.to_ascii_lowercase();
        self.tables.insert(key, TableData { schema, rows });
    }

    /// Returns total row count across all loaded tables.
    #[must_use]
    pub fn total_row_count(&self) -> usize {
        self.tables.values().map(|t| t.rows.len()).sum()
    }

    fn execute_single_statement(&mut self, tokens: &[TokenSpan]) -> Result<QueryResult, DbError> {
        let start = Instant::now();
        let mut cursor = TokenCursor::new(tokens);
        let first = cursor
            .peek_upper()
            .ok_or_else(|| DbError::Syntax("empty SQL statement".to_string()))?;

        let mut result = match first.as_str() {
            "EXPLAIN" => {
                cursor.next();
                self.exec_explain(&mut cursor)?
            }
            "SELECT" => self.exec_select(&mut cursor)?,
            "CREATE" => self.exec_create(&mut cursor)?,
            "INSERT" => self.exec_insert(&mut cursor)?,
            "UPDATE" => self.exec_update(&mut cursor)?,
            "DELETE" => self.exec_delete(&mut cursor)?,
            "DROP" => self.exec_drop(&mut cursor)?,
            "TRUNCATE" => self.exec_truncate(&mut cursor)?,
            other => {
                return Err(DbError::Syntax(format!(
                    "unsupported statement starting with '{other}'"
                )))
            }
        };

        result.execution_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok(result)
    }

    fn exec_explain(&self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        let stmt_kind = cursor
            .peek_upper()
            .ok_or_else(|| DbError::Syntax("EXPLAIN requires a target SQL statement".to_string()))?;

        let mut plan_rows = Vec::new();
        let mut step = 1i64;

        if stmt_kind == "SELECT" {
            let remaining = cursor.remaining_slice();
            let from_idx = remaining
                .iter()
                .position(|t| t.kind == TokenKind::Keyword && t.text.eq_ignore_ascii_case("FROM"));
            let table_name = from_idx
                .and_then(|idx| remaining.get(idx + 1))
                .map(|t| unquote_ident(&t.text))
                .unwrap_or_else(|| "unknown".to_string());
            let row_est = self
                .tables
                .get(&table_name.to_ascii_lowercase())
                .map_or(0, |t| t.rows.len() as i64);

            plan_rows.push(vec![
                Value::Integer(step),
                Value::Text("SeqScan".to_string()),
                Value::Text(table_name.clone()),
                Value::Integer(row_est),
                Value::Text(format!("Full in-memory scan over '{table_name}'")),
            ]);
            step += 1;

            if remaining
                .iter()
                .any(|t| t.kind == TokenKind::Keyword && t.text.eq_ignore_ascii_case("JOIN"))
            {
                plan_rows.push(vec![
                    Value::Integer(step),
                    Value::Text("HashJoin".to_string()),
                    Value::Text("joined_relation".to_string()),
                    Value::Integer(row_est),
                    Value::Text("Nested/equi-join probe over right-hand relation".to_string()),
                ]);
                step += 1;
            }

            if remaining
                .iter()
                .any(|t| t.kind == TokenKind::Keyword && t.text.eq_ignore_ascii_case("WHERE"))
            {
                plan_rows.push(vec![
                    Value::Integer(step),
                    Value::Text("Filter".to_string()),
                    Value::Text(table_name.clone()),
                    Value::Integer((row_est / 3).max(1)),
                    Value::Text("Evaluate WHERE predicate expression".to_string()),
                ]);
                step += 1;
            }

            if remaining
                .iter()
                .any(|t| t.kind == TokenKind::Keyword && t.text.eq_ignore_ascii_case("GROUP"))
            {
                plan_rows.push(vec![
                    Value::Integer(step),
                    Value::Text("HashAggregate".to_string()),
                    Value::Text("groups".to_string()),
                    Value::Integer((row_est / 6).max(1)),
                    Value::Text("Group rows and compute aggregate accumulators".to_string()),
                ]);
                step += 1;
            }

            if remaining
                .iter()
                .any(|t| t.kind == TokenKind::Keyword && t.text.eq_ignore_ascii_case("ORDER"))
            {
                plan_rows.push(vec![
                    Value::Integer(step),
                    Value::Text("Sort".to_string()),
                    Value::Text("projection".to_string()),
                    Value::Integer(row_est),
                    Value::Text("In-memory TimSort over ORDER BY keys".to_string()),
                ]);
                step += 1;
            }

            if remaining
                .iter()
                .any(|t| t.kind == TokenKind::Keyword && t.text.eq_ignore_ascii_case("LIMIT"))
            {
                plan_rows.push(vec![
                    Value::Integer(step),
                    Value::Text("Limit".to_string()),
                    Value::Text("stream".to_string()),
                    Value::Integer(row_est.min(50)),
                    Value::Text("Truncate output stream at LIMIT/OFFSET bounds".to_string()),
                ]);
            }
        } else {
            plan_rows.push(vec![
                Value::Integer(1),
                Value::Text(format!("Execute{stmt_kind}")),
                Value::Text("catalog".to_string()),
                Value::Integer(1),
                Value::Text(format!("Direct execution of {stmt_kind} statement")),
            ]);
        }

        let affected = plan_rows.len() as u64;
        Ok(QueryResult {
            columns: vec![
                ColumnMeta {
                    name: "step".to_string(),
                    data_type: "INTEGER".to_string(),
                    nullable: false,
                },
                ColumnMeta {
                    name: "operation".to_string(),
                    data_type: "TEXT".to_string(),
                    nullable: false,
                },
                ColumnMeta {
                    name: "target".to_string(),
                    data_type: "TEXT".to_string(),
                    nullable: false,
                },
                ColumnMeta {
                    name: "estimated_rows".to_string(),
                    data_type: "INTEGER".to_string(),
                    nullable: false,
                },
                ColumnMeta {
                    name: "detail".to_string(),
                    data_type: "TEXT".to_string(),
                    nullable: false,
                },
            ],
            rows: plan_rows,
            affected_rows: affected,
            execution_ms: 0.0,
            command_tag: "EXPLAIN".to_string(),
        })
    }

    fn exec_create(&mut self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        cursor.expect_keyword("CREATE")?;
        cursor.expect_keyword("TABLE")?;

        let mut if_not_exists = false;
        if cursor.consume_keyword("IF") {
            cursor.expect_keyword("NOT")?;
            cursor.expect_keyword("EXISTS")?;
            if_not_exists = true;
        }

        let table_name = cursor.expect_ident()?;
        let key = table_name.to_ascii_lowercase();
        if self.tables.contains_key(&key) {
            if if_not_exists {
                return Ok(empty_command_result("CREATE TABLE", 0));
            }
            return Err(DbError::Execution(format!(
                "table '{table_name}' already exists"
            )));
        }

        cursor.expect_punct("(")?;
        let mut columns = Vec::new();
        loop {
            if cursor.consume_punct(")") {
                break;
            }
            let col_name = cursor.expect_ident()?;
            let col_type = cursor
                .next()
                .map(|t| t.text.to_ascii_uppercase())
                .ok_or_else(|| DbError::Syntax("expected column type".to_string()))?;

            // Skip optional length spec such as VARCHAR(255)
            if cursor.consume_punct("(") {
                while let Some(t) = cursor.next() {
                    if t.kind == TokenKind::Punctuation && t.text == ")" {
                        break;
                    }
                }
            }

            let mut primary_key = false;
            let mut nullable = true;
            while let Some(upper) = cursor.peek_upper() {
                match upper.as_str() {
                    "PRIMARY" => {
                        cursor.next();
                        cursor.expect_keyword("KEY")?;
                        primary_key = true;
                        nullable = false;
                    }
                    "NOT" => {
                        cursor.next();
                        cursor.expect_keyword("NULL")?;
                        nullable = false;
                    }
                    "NULL" | "UNIQUE" => {
                        cursor.next();
                    }
                    _ => break,
                }
            }

            columns.push(ColumnSchema {
                name: col_name,
                data_type: col_type,
                nullable,
                primary_key,
                default_value: None,
            });

            if cursor.consume_punct(",") {
                continue;
            }
            cursor.expect_punct(")")?;
            break;
        }

        if columns.is_empty() {
            return Err(DbError::Syntax(
                "CREATE TABLE must define at least one column".to_string(),
            ));
        }

        self.insert_raw_table(
            TableSchema {
                name: table_name,
                schema: "public".to_string(),
                row_estimate: 0,
                columns,
            },
            Vec::new(),
        );

        Ok(empty_command_result("CREATE TABLE", 0))
    }

    fn exec_insert(&mut self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        cursor.expect_keyword("INSERT")?;
        cursor.expect_keyword("INTO")?;
        let table_name = cursor.expect_ident()?;
        let key = table_name.to_ascii_lowercase();

        let col_count_and_names = {
            let table = self
                .tables
                .get(&key)
                .ok_or_else(|| DbError::NotFound(format!("table '{table_name}' not found")))?;
            table
                .schema
                .columns
                .iter()
                .map(|c| c.name.to_ascii_lowercase())
                .collect::<Vec<_>>()
        };

        let target_indices: Vec<usize> = if cursor.consume_punct("(") {
            let mut indices = Vec::new();
            loop {
                let col_name = cursor.expect_ident()?.to_ascii_lowercase();
                let pos = col_count_and_names
                    .iter()
                    .position(|c| c == &col_name)
                    .ok_or_else(|| {
                        DbError::Execution(format!(
                            "column '{col_name}' does not exist on table '{table_name}'"
                        ))
                    })?;
                indices.push(pos);
                if cursor.consume_punct(",") {
                    continue;
                }
                cursor.expect_punct(")")?;
                break;
            }
            indices
        } else {
            (0..col_count_and_names.len()).collect()
        };

        cursor.expect_keyword("VALUES")?;
        let mut new_rows = Vec::new();
        loop {
            cursor.expect_punct("(")?;
            let mut vals = Vec::new();
            loop {
                vals.push(parse_literal(cursor)?);
                if cursor.consume_punct(",") {
                    continue;
                }
                cursor.expect_punct(")")?;
                break;
            }
            if vals.len() != target_indices.len() {
                return Err(DbError::Execution(format!(
                    "INSERT expected {} values, got {}",
                    target_indices.len(),
                    vals.len()
                )));
            }
            let mut full_row = vec![Value::Null; col_count_and_names.len()];
            for (val, &col_idx) in vals.into_iter().zip(&target_indices) {
                full_row[col_idx] = val;
            }
            new_rows.push(full_row);

            if !cursor.consume_punct(",") {
                break;
            }
        }

        let inserted = new_rows.len() as u64;
        let table = self
            .tables
            .get_mut(&key)
            .ok_or_else(|| DbError::NotFound(format!("table '{table_name}' not found")))?;
        table.rows.extend(new_rows);
        table.schema.row_estimate = table.rows.len() as u64;

        Ok(empty_command_result(&format!("INSERT 0 {inserted}"), inserted))
    }

    fn exec_update(&mut self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        cursor.expect_keyword("UPDATE")?;
        let table_name = cursor.expect_ident()?;
        let key = table_name.to_ascii_lowercase();

        cursor.expect_keyword("SET")?;
        let mut assignments: Vec<(String, Value)> = Vec::new();
        loop {
            let col_name = cursor.expect_ident()?;
            cursor.expect_op("=")?;
            let val = parse_literal(cursor)?;
            assignments.push((col_name, val));
            if !cursor.consume_punct(",") {
                break;
            }
        }

        let where_expr = if cursor.consume_keyword("WHERE") {
            Some(parse_expr(cursor)?)
        } else {
            None
        };

        let table = self
            .tables
            .get_mut(&key)
            .ok_or_else(|| DbError::NotFound(format!("table '{table_name}' not found")))?;

        let ctx = RowContext::from_single(&table.schema, None);
        let resolved_assignments: Vec<(usize, Value)> = assignments
            .into_iter()
            .map(|(col_name, val)| {
                let idx = ctx.resolve_col(&col_name).ok_or_else(|| {
                    DbError::Execution(format!(
                        "column '{col_name}' does not exist on table '{table_name}'"
                    ))
                })?;
                Ok((idx, val))
            })
            .collect::<Result<Vec<_>, DbError>>()?;

        let mut updated = 0u64;
        for row in &mut table.rows {
            let matches = match &where_expr {
                Some(expr) => eval_expr(expr, row, &ctx)?,
                None => true,
            };
            if matches {
                for (idx, val) in &resolved_assignments {
                    row[*idx] = val.clone();
                }
                updated += 1;
            }
        }

        Ok(empty_command_result(&format!("UPDATE {updated}"), updated))
    }

    fn exec_delete(&mut self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        cursor.expect_keyword("DELETE")?;
        cursor.expect_keyword("FROM")?;
        let table_name = cursor.expect_ident()?;
        let key = table_name.to_ascii_lowercase();

        let where_expr = if cursor.consume_keyword("WHERE") {
            Some(parse_expr(cursor)?)
        } else {
            None
        };

        let table = self
            .tables
            .get_mut(&key)
            .ok_or_else(|| DbError::NotFound(format!("table '{table_name}' not found")))?;

        let ctx = RowContext::from_single(&table.schema, None);
        let original_len = table.rows.len();
        let mut kept = Vec::with_capacity(original_len);
        for row in table.rows.drain(..) {
            let should_delete = match &where_expr {
                Some(expr) => eval_expr(expr, &row, &ctx)?,
                None => true,
            };
            if !should_delete {
                kept.push(row);
            }
        }
        let deleted = (original_len - kept.len()) as u64;
        table.rows = kept;
        table.schema.row_estimate = table.rows.len() as u64;

        Ok(empty_command_result(&format!("DELETE {deleted}"), deleted))
    }

    fn exec_drop(&mut self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        cursor.expect_keyword("DROP")?;
        cursor.expect_keyword("TABLE")?;
        let mut if_exists = false;
        if cursor.consume_keyword("IF") {
            cursor.expect_keyword("EXISTS")?;
            if_exists = true;
        }
        let table_name = cursor.expect_ident()?;
        let key = table_name.to_ascii_lowercase();
        if self.tables.remove(&key).is_none() && !if_exists {
            return Err(DbError::NotFound(format!("table '{table_name}' not found")));
        }
        Ok(empty_command_result("DROP TABLE", 0))
    }

    fn exec_truncate(&mut self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        cursor.expect_keyword("TRUNCATE")?;
        let _ = cursor.consume_keyword("TABLE");
        let table_name = cursor.expect_ident()?;
        let key = table_name.to_ascii_lowercase();
        let table = self
            .tables
            .get_mut(&key)
            .ok_or_else(|| DbError::NotFound(format!("table '{table_name}' not found")))?;
        let cleared = table.rows.len() as u64;
        table.rows.clear();
        table.schema.row_estimate = 0;
        Ok(empty_command_result("TRUNCATE TABLE", cleared))
    }

    fn exec_select(&self, cursor: &mut TokenCursor<'_>) -> Result<QueryResult, DbError> {
        cursor.expect_keyword("SELECT")?;
        let distinct = cursor.consume_keyword("DISTINCT");

        let projections = parse_select_projections(cursor)?;
        cursor.expect_keyword("FROM")?;

        let base_table_name = cursor.expect_ident()?;
        let base_alias = parse_optional_alias(cursor);
        let base_table = self
            .tables
            .get(&base_table_name.to_ascii_lowercase())
            .ok_or_else(|| DbError::NotFound(format!("table '{base_table_name}' not found")))?;

        let mut ctx = RowContext::from_single(&base_table.schema, base_alias.as_deref());
        let mut working_rows: Vec<Vec<Value>> = base_table.rows.clone();

        // Optional INNER / LEFT JOIN clauses
        while let Some(join_kind) = parse_join_kind(cursor) {
            let right_name = cursor.expect_ident()?;
            let right_alias = parse_optional_alias(cursor);
            let right_table = self
                .tables
                .get(&right_name.to_ascii_lowercase())
                .ok_or_else(|| DbError::NotFound(format!("table '{right_name}' not found")))?;

            cursor.expect_keyword("ON")?;
            let left_col_ref = parse_column_ref(cursor)?;
            cursor.expect_op("=")?;
            let right_col_ref = parse_column_ref(cursor)?;

            let left_len = ctx.columns.len();
            let right_ctx = RowContext::from_single(&right_table.schema, right_alias.as_deref());
            let mut merged_ctx = ctx.clone();
            merged_ctx.append(&right_ctx);

            let left_idx = merged_ctx.resolve_col(&left_col_ref).ok_or_else(|| {
                DbError::Execution(format!("unknown JOIN column '{left_col_ref}'"))
            })?;
            let right_idx = merged_ctx.resolve_col(&right_col_ref).ok_or_else(|| {
                DbError::Execution(format!("unknown JOIN column '{right_col_ref}'"))
            })?;

            let mut joined = Vec::new();
            for lrow in &working_rows {
                let mut matched = false;
                for rrow in &right_table.rows {
                    let mut combined = Vec::with_capacity(left_len + rrow.len());
                    combined.extend_from_slice(lrow);
                    combined.extend_from_slice(rrow);
                    if !combined[left_idx].is_null()
                        && combined[left_idx] == combined[right_idx]
                    {
                        joined.push(combined);
                        matched = true;
                    }
                }
                if !matched && join_kind == JoinKind::Left {
                    let mut combined = Vec::with_capacity(left_len + right_table.schema.columns.len());
                    combined.extend_from_slice(lrow);
                    combined.resize(left_len + right_table.schema.columns.len(), Value::Null);
                    joined.push(combined);
                }
            }

            ctx = merged_ctx;
            working_rows = joined;
        }

        // Optional WHERE filter
        if cursor.consume_keyword("WHERE") {
            let expr = parse_expr(cursor)?;
            let mut filtered = Vec::new();
            for row in working_rows {
                if eval_expr(&expr, &row, &ctx)? {
                    filtered.push(row);
                }
            }
            working_rows = filtered;
        }

        // Optional GROUP BY
        let mut group_by_cols: Vec<String> = Vec::new();
        if cursor.consume_keyword("GROUP") {
            cursor.expect_keyword("BY")?;
            loop {
                group_by_cols.push(parse_column_ref(cursor)?);
                if !cursor.consume_punct(",") {
                    break;
                }
            }
        }

        // Optional ORDER BY
        let mut order_by: Vec<(String, bool)> = Vec::new();
        if cursor.consume_keyword("ORDER") {
            cursor.expect_keyword("BY")?;
            loop {
                let col_ref = parse_column_ref(cursor)?;
                let descending = if cursor.consume_keyword("DESC") {
                    true
                } else {
                    let _ = cursor.consume_keyword("ASC");
                    false
                };
                order_by.push((col_ref, descending));
                if !cursor.consume_punct(",") {
                    break;
                }
            }
        }

        // Optional LIMIT / OFFSET
        let limit = if cursor.consume_keyword("LIMIT") {
            Some(parse_usize(cursor)?)
        } else {
            None
        };
        let offset = if cursor.consume_keyword("OFFSET") {
            parse_usize(cursor)?
        } else {
            0
        };

        let has_aggregates = projections
            .iter()
            .any(|p| matches!(p, ProjectionItem::Aggregate { .. }));

        let (out_columns, mut out_rows) = if has_aggregates || !group_by_cols.is_empty() {
            execute_aggregate_projection(&projections, &group_by_cols, &working_rows, &ctx)?
        } else {
            // Sort before projection so ORDER BY can reference any table column
            if !order_by.is_empty() {
                let sort_specs: Vec<(usize, bool)> = order_by
                    .iter()
                    .map(|(name, desc)| {
                        let idx = ctx.resolve_col(name).ok_or_else(|| {
                            DbError::Execution(format!("ORDER BY column '{name}' not found"))
                        })?;
                        Ok((idx, *desc))
                    })
                    .collect::<Result<Vec<_>, DbError>>()?;

                working_rows.sort_by(|a, b| {
                    for &(idx, desc) in &sort_specs {
                        let cmp = a[idx].sql_cmp(&b[idx]);
                        if cmp != std::cmp::Ordering::Equal {
                            return if desc { cmp.reverse() } else { cmp };
                        }
                    }
                    std::cmp::Ordering::Equal
                });
            }
            project_rows(&projections, &working_rows, &ctx)?
        };

        // If grouped/aggregated and ORDER BY specified, sort against projected column names
        if (has_aggregates || !group_by_cols.is_empty()) && !order_by.is_empty() {
            let sort_specs: Vec<(usize, bool)> = order_by
                .iter()
                .map(|(name, desc)| {
                    let target = name.to_ascii_lowercase();
                    let idx = out_columns
                        .iter()
                        .position(|c| c.name.to_ascii_lowercase() == target)
                        .ok_or_else(|| {
                            DbError::Execution(format!(
                                "ORDER BY column '{name}' not found in aggregate projection"
                            ))
                        })?;
                    Ok((idx, *desc))
                })
                .collect::<Result<Vec<_>, DbError>>()?;

            out_rows.sort_by(|a, b| {
                for &(idx, desc) in &sort_specs {
                    let cmp = a[idx].sql_cmp(&b[idx]);
                    if cmp != std::cmp::Ordering::Equal {
                        return if desc { cmp.reverse() } else { cmp };
                    }
                }
                std::cmp::Ordering::Equal
            });
        }

        if distinct {
            let mut seen: HashSet<String> = HashSet::new();
            out_rows.retain(|row| {
                let key = serde_json::to_string(row).unwrap_or_default();
                seen.insert(key)
            });
        }

        if offset > 0 {
            if offset >= out_rows.len() {
                out_rows.clear();
            } else {
                out_rows.drain(0..offset);
            }
        }
        if let Some(lim) = limit {
            out_rows.truncate(lim);
        }

        let count = out_rows.len() as u64;
        Ok(QueryResult {
            columns: out_columns,
            rows: out_rows,
            affected_rows: count,
            execution_ms: 0.0,
            command_tag: format!("SELECT {count}"),
        })
    }
}

impl DatabaseDriver for LocalEngine {
    fn driver_id(&self) -> &'static str {
        "local-engine"
    }

    fn connect(&mut self, profile: &ConnectionProfile) -> Result<(), DbError> {
        self.database_name = profile.database.clone();
        self.connected = true;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn list_tables(&mut self) -> Result<Vec<TableSchema>, DbError> {
        Ok(self.tables.values().map(|t| t.schema.clone()).collect())
    }

    fn describe_table(&mut self, table_name: &str) -> Result<TableSchema, DbError> {
        self.tables
            .get(&table_name.to_ascii_lowercase())
            .map(|t| t.schema.clone())
            .ok_or_else(|| DbError::NotFound(format!("table '{table_name}' not found")))
    }

    fn execute_query(&mut self, sql: &str) -> Result<QueryResult, DbError> {
        let all: Vec<TokenSpan> = tokenize_sql(sql)
            .into_iter()
            .filter(|t| {
                !matches!(
                    t.kind,
                    TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment
                )
            })
            .collect();

        if all.is_empty() {
            return Err(DbError::Syntax("SQL query is empty".to_string()));
        }

        // Split by top-level semicolons and execute sequentially, returning the last result
        let mut statements: Vec<&[TokenSpan]> = Vec::new();
        let mut start = 0usize;
        let mut depth = 0usize;
        for (i, tok) in all.iter().enumerate() {
            if tok.kind == TokenKind::Punctuation {
                if tok.text == "(" {
                    depth += 1;
                } else if tok.text == ")" {
                    depth = depth.saturating_sub(1);
                } else if tok.text == ";" && depth == 0 {
                    if i > start {
                        statements.push(&all[start..i]);
                    }
                    start = i + 1;
                }
            }
        }
        if start < all.len() {
            statements.push(&all[start..]);
        }

        let mut last_result = None;
        for stmt_tokens in statements {
            last_result = Some(self.execute_single_statement(stmt_tokens)?);
        }

        last_result.ok_or_else(|| DbError::Syntax("no executable SQL statement found".to_string()))
    }

    fn stream_export(
        &mut self,
        sql: &str,
        options: &ExportOptions,
        writer: &mut dyn Write,
    ) -> Result<ExportStats, DbError> {
        let result = self.execute_query(sql)?;
        stream_export_rows(&result.columns, result.rows, options, writer)
    }
}

fn col_pk(name: &str, data_type: &str) -> ColumnSchema {
    ColumnSchema {
        name: name.to_string(),
        data_type: data_type.to_string(),
        nullable: false,
        primary_key: true,
        default_value: None,
    }
}

fn col(name: &str, data_type: &str, nullable: bool) -> ColumnSchema {
    ColumnSchema {
        name: name.to_string(),
        data_type: data_type.to_string(),
        nullable,
        primary_key: false,
        default_value: None,
    }
}

fn empty_command_result(tag: &str, affected: u64) -> QueryResult {
    QueryResult {
        columns: vec![
            ColumnMeta {
                name: "status".to_string(),
                data_type: "TEXT".to_string(),
                nullable: false,
            },
            ColumnMeta {
                name: "affected_rows".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
            },
        ],
        rows: vec![vec![
            Value::Text(tag.to_string()),
            Value::Integer(affected as i64),
        ]],
        affected_rows: affected,
        execution_ms: 0.0,
        command_tag: tag.to_string(),
    }
}

// --- Internal parser helpers ---

struct TokenCursor<'a> {
    tokens: &'a [TokenSpan],
    pos: usize,
}

impl<'a> TokenCursor<'a> {
    fn new(tokens: &'a [TokenSpan]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&'a TokenSpan> {
        self.tokens.get(self.pos)
    }

    fn peek_upper(&self) -> Option<String> {
        self.peek().map(|t| t.text.to_ascii_uppercase())
    }

    fn next(&mut self) -> Option<&'a TokenSpan> {
        let tok = self.tokens.get(self.pos)?;
        self.pos += 1;
        Some(tok)
    }

    fn remaining_slice(&self) -> &'a [TokenSpan] {
        &self.tokens[self.pos..]
    }

    fn consume_keyword(&mut self, kw: &str) -> bool {
        if self
            .peek()
            .is_some_and(|t| t.kind == TokenKind::Keyword && t.text.eq_ignore_ascii_case(kw))
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), DbError> {
        if self.consume_keyword(kw) {
            Ok(())
        } else {
            let found = self
                .peek()
                .map_or_else(|| "EOF".to_string(), |t| t.text.clone());
            Err(DbError::Syntax(format!(
                "expected keyword '{kw}', found '{found}'"
            )))
        }
    }

    fn consume_punct(&mut self, p: &str) -> bool {
        if self
            .peek()
            .is_some_and(|t| t.kind == TokenKind::Punctuation && t.text == p)
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), DbError> {
        if self.consume_punct(p) {
            Ok(())
        } else {
            let found = self
                .peek()
                .map_or_else(|| "EOF".to_string(), |t| t.text.clone());
            Err(DbError::Syntax(format!(
                "expected '{p}', found '{found}'"
            )))
        }
    }

    fn expect_op(&mut self, op: &str) -> Result<(), DbError> {
        if self
            .peek()
            .is_some_and(|t| t.kind == TokenKind::Operator && t.text == op)
        {
            self.pos += 1;
            Ok(())
        } else {
            let found = self
                .peek()
                .map_or_else(|| "EOF".to_string(), |t| t.text.clone());
            Err(DbError::Syntax(format!(
                "expected operator '{op}', found '{found}'"
            )))
        }
    }

    fn expect_ident(&mut self) -> Result<String, DbError> {
        let tok = self
            .next()
            .ok_or_else(|| DbError::Syntax("expected identifier, found EOF".to_string()))?;
        match tok.kind {
            TokenKind::Identifier | TokenKind::QuotedIdentifier | TokenKind::Keyword => {
                Ok(unquote_ident(&tok.text))
            }
            _ => Err(DbError::Syntax(format!(
                "expected identifier, found '{}'",
                tok.text
            ))),
        }
    }
}

fn unquote_ident(raw: &str) -> String {
    if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('`') && raw.ends_with('`')))
    {
        return raw[1..raw.len() - 1].replace("\"\"", "\"");
    }
    raw.to_string()
}

fn unquote_string(raw: &str) -> String {
    if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
        raw[1..raw.len() - 1].replace("''", "'")
    } else {
        raw.to_string()
    }
}

fn parse_literal(cursor: &mut TokenCursor<'_>) -> Result<Value, DbError> {
    let negative = cursor
        .peek()
        .is_some_and(|t| t.kind == TokenKind::Operator && t.text == "-");
    if negative {
        cursor.next();
    }

    let tok = cursor
        .next()
        .ok_or_else(|| DbError::Syntax("expected literal value, found EOF".to_string()))?;

    match tok.kind {
        TokenKind::Number => {
            let num_text = if negative {
                format!("-{}", tok.text)
            } else {
                tok.text.clone()
            };
            if num_text.contains('.') || num_text.contains('e') || num_text.contains('E') {
                let f: f64 = num_text
                    .parse()
                    .map_err(|_| DbError::Syntax(format!("invalid float literal '{num_text}'")))?;
                Ok(Value::Float(f))
            } else {
                let i: i64 = num_text
                    .parse()
                    .map_err(|_| DbError::Syntax(format!("invalid integer literal '{num_text}'")))?;
                Ok(Value::Integer(i))
            }
        }
        TokenKind::StringLiteral => Ok(Value::Text(unquote_string(&tok.text))),
        TokenKind::Keyword => match tok.text.to_ascii_uppercase().as_str() {
            "NULL" => Ok(Value::Null),
            "TRUE" => Ok(Value::Bool(true)),
            "FALSE" => Ok(Value::Bool(false)),
            other => Err(DbError::Syntax(format!(
                "unexpected keyword '{other}' where literal was expected"
            ))),
        },
        _ => Err(DbError::Syntax(format!(
            "expected literal value, found '{}'",
            tok.text
        ))),
    }
}

fn parse_usize(cursor: &mut TokenCursor<'_>) -> Result<usize, DbError> {
    let tok = cursor
        .next()
        .ok_or_else(|| DbError::Syntax("expected positive integer".to_string()))?;
    if tok.kind != TokenKind::Number {
        return Err(DbError::Syntax(format!(
            "expected integer, found '{}'",
            tok.text
        )));
    }
    tok.text
        .parse::<usize>()
        .map_err(|_| DbError::Syntax(format!("invalid integer '{}'", tok.text)))
}

fn parse_column_ref(cursor: &mut TokenCursor<'_>) -> Result<String, DbError> {
    let first = cursor.expect_ident()?;
    if cursor.consume_punct(".") {
        let second = cursor.expect_ident()?;
        Ok(format!("{first}.{second}"))
    } else {
        Ok(first)
    }
}

fn parse_optional_alias(cursor: &mut TokenCursor<'_>) -> Option<String> {
    if cursor.consume_keyword("AS") {
        return cursor.expect_ident().ok();
    }
    if let Some(tok) = cursor.peek() {
        if tok.kind == TokenKind::Identifier || tok.kind == TokenKind::QuotedIdentifier {
            return cursor.expect_ident().ok();
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinKind {
    Inner,
    Left,
}

fn parse_join_kind(cursor: &mut TokenCursor<'_>) -> Option<JoinKind> {
    if cursor.consume_keyword("INNER") {
        let _ = cursor.consume_keyword("JOIN");
        Some(JoinKind::Inner)
    } else if cursor.consume_keyword("LEFT") {
        let _ = cursor.consume_keyword("OUTER");
        let _ = cursor.consume_keyword("JOIN");
        Some(JoinKind::Left)
    } else if cursor.consume_keyword("JOIN") {
        Some(JoinKind::Inner)
    } else {
        None
    }
}

#[derive(Debug, Clone)]
enum AggregateFunc {
    Count,
    Sum,
    Avg,
    Min,
    Max,
}

#[derive(Debug, Clone)]
enum ProjectionItem {
    Star(Option<String>),
    Column {
        col_ref: String,
        alias: Option<String>,
    },
    Aggregate {
        func: AggregateFunc,
        arg: Option<String>,
        alias: Option<String>,
    },
}

fn parse_select_projections(cursor: &mut TokenCursor<'_>) -> Result<Vec<ProjectionItem>, DbError> {
    let mut items = Vec::new();
    loop {
        if cursor
            .peek()
            .is_some_and(|t| t.kind == TokenKind::Operator && t.text == "*")
        {
            cursor.next();
            items.push(ProjectionItem::Star(None));
        } else if let Some(upper) = cursor.peek_upper() {
            let agg = match upper.as_str() {
                "COUNT" => Some(AggregateFunc::Count),
                "SUM" => Some(AggregateFunc::Sum),
                "AVG" => Some(AggregateFunc::Avg),
                "MIN" => Some(AggregateFunc::Min),
                "MAX" => Some(AggregateFunc::Max),
                _ => None,
            };
            if let Some(func) = agg {
                cursor.next();
                cursor.expect_punct("(")?;
                let arg = if cursor
                    .peek()
                    .is_some_and(|t| t.kind == TokenKind::Operator && t.text == "*")
                {
                    cursor.next();
                    None
                } else {
                    Some(parse_column_ref(cursor)?)
                };
                cursor.expect_punct(")")?;
                let alias = parse_optional_alias(cursor);
                items.push(ProjectionItem::Aggregate { func, arg, alias });
            } else {
                let first = cursor.expect_ident()?;
                if cursor.consume_punct(".") {
                    if cursor
                        .peek()
                        .is_some_and(|t| t.kind == TokenKind::Operator && t.text == "*")
                    {
                        cursor.next();
                        items.push(ProjectionItem::Star(Some(first)));
                    } else {
                        let second = cursor.expect_ident()?;
                        let alias = parse_optional_alias(cursor);
                        items.push(ProjectionItem::Column {
                            col_ref: format!("{first}.{second}"),
                            alias,
                        });
                    }
                } else {
                    let alias = parse_optional_alias(cursor);
                    items.push(ProjectionItem::Column {
                        col_ref: first,
                        alias,
                    });
                }
            }
        } else {
            return Err(DbError::Syntax("unexpected end of SELECT list".to_string()));
        }

        if !cursor.consume_punct(",") {
            break;
        }
    }
    Ok(items)
}

#[derive(Debug, Clone)]
struct RowContext {
    columns: Vec<(String, String, ColumnMeta)>, // (table_qualifier, col_name, meta)
}

impl RowContext {
    fn from_single(schema: &TableSchema, alias: Option<&str>) -> Self {
        let qualifier = alias.unwrap_or(&schema.name).to_ascii_lowercase();
        let columns = schema
            .columns
            .iter()
            .map(|c| {
                (
                    qualifier.clone(),
                    c.name.to_ascii_lowercase(),
                    ColumnMeta {
                        name: c.name.clone(),
                        data_type: c.data_type.clone(),
                        nullable: c.nullable,
                    },
                )
            })
            .collect();
        Self { columns }
    }

    fn append(&mut self, other: &Self) {
        self.columns.extend(other.columns.clone());
    }

    fn resolve_col(&self, col_ref: &str) -> Option<usize> {
        let lower = col_ref.to_ascii_lowercase();
        if let Some((qual, col)) = lower.split_once('.') {
            self.columns
                .iter()
                .position(|(q, c, _)| q == qual && c == col)
        } else {
            self.columns.iter().position(|(_, c, _)| c == &lower)
        }
    }
}

fn project_rows(
    projections: &[ProjectionItem],
    rows: &[Vec<Value>],
    ctx: &RowContext,
) -> Result<(Vec<ColumnMeta>, Vec<Vec<Value>>), DbError> {
    let mut selected_indices: Vec<usize> = Vec::new();
    let mut out_cols: Vec<ColumnMeta> = Vec::new();

    for item in projections {
        match item {
            ProjectionItem::Star(None) => {
                for (idx, (_, _, meta)) in ctx.columns.iter().enumerate() {
                    selected_indices.push(idx);
                    out_cols.push(meta.clone());
                }
            }
            ProjectionItem::Star(Some(qual)) => {
                let q_lower = qual.to_ascii_lowercase();
                for (idx, (q, _, meta)) in ctx.columns.iter().enumerate() {
                    if q == &q_lower {
                        selected_indices.push(idx);
                        out_cols.push(meta.clone());
                    }
                }
            }
            ProjectionItem::Column { col_ref, alias } => {
                let idx = ctx
                    .resolve_col(col_ref)
                    .ok_or_else(|| DbError::Execution(format!("unknown column '{col_ref}'")))?;
                let mut meta = ctx.columns[idx].2.clone();
                if let Some(a) = alias {
                    meta.name = a.clone();
                }
                selected_indices.push(idx);
                out_cols.push(meta);
            }
            ProjectionItem::Aggregate { .. } => {
                return Err(DbError::Execution(
                    "aggregate projection routed to non-aggregate path".to_string(),
                ));
            }
        }
    }

    let out_rows = rows
        .iter()
        .map(|row| selected_indices.iter().map(|&idx| row[idx].clone()).collect())
        .collect();

    Ok((out_cols, out_rows))
}

fn execute_aggregate_projection(
    projections: &[ProjectionItem],
    group_by_cols: &[String],
    rows: &[Vec<Value>],
    ctx: &RowContext,
) -> Result<(Vec<ColumnMeta>, Vec<Vec<Value>>), DbError> {
    let group_indices: Vec<usize> = group_by_cols
        .iter()
        .map(|c| {
            ctx.resolve_col(c)
                .ok_or_else(|| DbError::Execution(format!("GROUP BY column '{c}' not found")))
        })
        .collect::<Result<Vec<_>, DbError>>()?;

    let mut groups: BTreeMap<String, Vec<&Vec<Value>>> = BTreeMap::new();
    if group_indices.is_empty() {
        groups.insert(String::new(), rows.iter().collect());
    } else {
        for row in rows {
            let key_vals: Vec<&Value> = group_indices.iter().map(|&i| &row[i]).collect();
            let key = serde_json::to_string(&key_vals).unwrap_or_default();
            groups.entry(key).or_default().push(row);
        }
    }

    let mut out_cols = Vec::new();
    for item in projections {
        match item {
            ProjectionItem::Column { col_ref, alias } => {
                let idx = ctx
                    .resolve_col(col_ref)
                    .ok_or_else(|| DbError::Execution(format!("unknown column '{col_ref}'")))?;
                let mut meta = ctx.columns[idx].2.clone();
                if let Some(a) = alias {
                    meta.name = a.clone();
                }
                out_cols.push(meta);
            }
            ProjectionItem::Aggregate { func, arg, alias } => {
                let default_name = match (func, arg) {
                    (AggregateFunc::Count, None) => "count".to_string(),
                    (AggregateFunc::Count, Some(c)) => format!("count_{c}"),
                    (AggregateFunc::Sum, Some(c)) => format!("sum_{c}"),
                    (AggregateFunc::Avg, Some(c)) => format!("avg_{c}"),
                    (AggregateFunc::Min, Some(c)) => format!("min_{c}"),
                    (AggregateFunc::Max, Some(c)) => format!("max_{c}"),
                    _ => "agg".to_string(),
                };
                let data_type = match func {
                    AggregateFunc::Count => "INTEGER",
                    AggregateFunc::Avg | AggregateFunc::Sum => "REAL",
                    AggregateFunc::Min | AggregateFunc::Max => "TEXT",
                };
                out_cols.push(ColumnMeta {
                    name: alias.clone().unwrap_or(default_name),
                    data_type: data_type.to_string(),
                    nullable: true,
                });
            }
            ProjectionItem::Star(_) => {
                return Err(DbError::Syntax(
                    "SELECT * cannot be combined with aggregates".to_string(),
                ));
            }
        }
    }

    let mut out_rows = Vec::with_capacity(groups.len());
    for group_rows in groups.values() {
        let mut row_out = Vec::with_capacity(projections.len());
        for item in projections {
            match item {
                ProjectionItem::Column { col_ref, .. } => {
                    let idx = ctx.resolve_col(col_ref).ok_or_else(|| {
                        DbError::Execution(format!("unknown column '{col_ref}'"))
                    })?;
                    let val = group_rows
                        .first()
                        .map_or(Value::Null, |r| r[idx].clone());
                    row_out.push(val);
                }
                ProjectionItem::Aggregate { func, arg, .. } => {
                    let val = compute_aggregate(func, arg.as_deref(), group_rows, ctx)?;
                    row_out.push(val);
                }
                ProjectionItem::Star(_) => {}
            }
        }
        out_rows.push(row_out);
    }

    Ok((out_cols, out_rows))
}

fn compute_aggregate(
    func: &AggregateFunc,
    arg: Option<&str>,
    rows: &[&Vec<Value>],
    ctx: &RowContext,
) -> Result<Value, DbError> {
    match func {
        AggregateFunc::Count => match arg {
            None => Ok(Value::Integer(rows.len() as i64)),
            Some(col_ref) => {
                let idx = ctx
                    .resolve_col(col_ref)
                    .ok_or_else(|| DbError::Execution(format!("unknown column '{col_ref}'")))?;
                let count = rows.iter().filter(|r| !r[idx].is_null()).count() as i64;
                Ok(Value::Integer(count))
            }
        },
        AggregateFunc::Sum => {
            let col_ref = arg.ok_or_else(|| DbError::Syntax("SUM requires a column".to_string()))?;
            let idx = ctx
                .resolve_col(col_ref)
                .ok_or_else(|| DbError::Execution(format!("unknown column '{col_ref}'")))?;
            let mut sum = 0.0f64;
            let mut any = false;
            for r in rows {
                if let Some(n) = r[idx].as_f64() {
                    sum += n;
                    any = true;
                }
            }
            if !any {
                Ok(Value::Null)
            } else if sum.fract() == 0.0 {
                Ok(Value::Integer(sum as i64))
            } else {
                Ok(Value::Float((sum * 100.0).round() / 100.0))
            }
        }
        AggregateFunc::Avg => {
            let col_ref = arg.ok_or_else(|| DbError::Syntax("AVG requires a column".to_string()))?;
            let idx = ctx
                .resolve_col(col_ref)
                .ok_or_else(|| DbError::Execution(format!("unknown column '{col_ref}'")))?;
            let mut sum = 0.0f64;
            let mut count = 0usize;
            for r in rows {
                if let Some(n) = r[idx].as_f64() {
                    sum += n;
                    count += 1;
                }
            }
            if count == 0 {
                Ok(Value::Null)
            } else {
                Ok(Value::Float(((sum / count as f64) * 100.0).round() / 100.0))
            }
        }
        AggregateFunc::Min | AggregateFunc::Max => {
            let col_ref = arg.ok_or_else(|| DbError::Syntax("MIN/MAX requires a column".to_string()))?;
            let idx = ctx
                .resolve_col(col_ref)
                .ok_or_else(|| DbError::Execution(format!("unknown column '{col_ref}'")))?;
            let mut best: Option<&Value> = None;
            for r in rows {
                let v = &r[idx];
                if v.is_null() {
                    continue;
                }
                best = match best {
                    None => Some(v),
                    Some(cur) => {
                        let cmp = v.sql_cmp(cur);
                        let better = matches!(
                            (func, cmp),
                            (AggregateFunc::Min, std::cmp::Ordering::Less)
                                | (AggregateFunc::Max, std::cmp::Ordering::Greater)
                        );
                        if better {
                            Some(v)
                        } else {
                            Some(cur)
                        }
                    }
                };
            }
            Ok(best.cloned().unwrap_or(Value::Null))
        }
    }
}

// --- WHERE Expression AST & Evaluator ---

#[derive(Debug, Clone)]
enum Expr {
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Compare {
        col: String,
        op: String,
        rhs: Value,
    },
    Like {
        col: String,
        pattern: String,
        negated: bool,
    },
    InList {
        col: String,
        values: Vec<Value>,
        negated: bool,
    },
    IsNull {
        col: String,
        negated: bool,
    },
}

fn parse_expr(cursor: &mut TokenCursor<'_>) -> Result<Expr, DbError> {
    parse_or_expr(cursor)
}

fn parse_or_expr(cursor: &mut TokenCursor<'_>) -> Result<Expr, DbError> {
    let mut left = parse_and_expr(cursor)?;
    while cursor.consume_keyword("OR") {
        let right = parse_and_expr(cursor)?;
        left = Expr::Or(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_and_expr(cursor: &mut TokenCursor<'_>) -> Result<Expr, DbError> {
    let mut left = parse_unary_expr(cursor)?;
    while cursor.consume_keyword("AND") {
        let right = parse_unary_expr(cursor)?;
        left = Expr::And(Box::new(left), Box::new(right));
    }
    Ok(left)
}

fn parse_unary_expr(cursor: &mut TokenCursor<'_>) -> Result<Expr, DbError> {
    if cursor.consume_keyword("NOT") {
        let inner = parse_unary_expr(cursor)?;
        return Ok(Expr::Not(Box::new(inner)));
    }
    if cursor.consume_punct("(") {
        let inner = parse_expr(cursor)?;
        cursor.expect_punct(")")?;
        return Ok(inner);
    }

    let col = parse_column_ref(cursor)?;

    if cursor.consume_keyword("IS") {
        let negated = cursor.consume_keyword("NOT");
        cursor.expect_keyword("NULL")?;
        return Ok(Expr::IsNull { col, negated });
    }

    let negated = cursor.consume_keyword("NOT");

    if cursor.consume_keyword("LIKE") || cursor.consume_keyword("ILIKE") {
        let val = parse_literal(cursor)?;
        let pattern = match val {
            Value::Text(s) => s,
            other => other.display_string(),
        };
        return Ok(Expr::Like {
            col,
            pattern,
            negated,
        });
    }

    if cursor.consume_keyword("IN") {
        cursor.expect_punct("(")?;
        let mut values = Vec::new();
        loop {
            values.push(parse_literal(cursor)?);
            if cursor.consume_punct(",") {
                continue;
            }
            cursor.expect_punct(")")?;
            break;
        }
        return Ok(Expr::InList {
            col,
            values,
            negated,
        });
    }

    let op_tok = cursor
        .next()
        .ok_or_else(|| DbError::Syntax("expected comparison operator in WHERE".to_string()))?;
    if op_tok.kind != TokenKind::Operator {
        return Err(DbError::Syntax(format!(
            "expected comparison operator, found '{}'",
            op_tok.text
        )));
    }
    let rhs = parse_literal(cursor)?;
    Ok(Expr::Compare {
        col,
        op: op_tok.text.clone(),
        rhs,
    })
}

fn eval_expr(expr: &Expr, row: &[Value], ctx: &RowContext) -> Result<bool, DbError> {
    match expr {
        Expr::And(a, b) => Ok(eval_expr(a, row, ctx)? && eval_expr(b, row, ctx)?),
        Expr::Or(a, b) => Ok(eval_expr(a, row, ctx)? || eval_expr(b, row, ctx)?),
        Expr::Not(inner) => Ok(!eval_expr(inner, row, ctx)?),
        Expr::IsNull { col, negated } => {
            let idx = ctx
                .resolve_col(col)
                .ok_or_else(|| DbError::Execution(format!("unknown column '{col}'")))?;
            let is_null = row[idx].is_null();
            Ok(if *negated { !is_null } else { is_null })
        }
        Expr::Like {
            col,
            pattern,
            negated,
        } => {
            let idx = ctx
                .resolve_col(col)
                .ok_or_else(|| DbError::Execution(format!("unknown column '{col}'")))?;
            let matched = match &row[idx] {
                Value::Text(s) => sql_like_match(s, pattern),
                Value::Null => false,
                other => sql_like_match(&other.display_string(), pattern),
            };
            Ok(if *negated { !matched } else { matched })
        }
        Expr::InList {
            col,
            values,
            negated,
        } => {
            let idx = ctx
                .resolve_col(col)
                .ok_or_else(|| DbError::Execution(format!("unknown column '{col}'")))?;
            let val = &row[idx];
            if val.is_null() {
                return Ok(false);
            }
            let found = values
                .iter()
                .any(|candidate| val.sql_cmp(candidate) == std::cmp::Ordering::Equal);
            Ok(if *negated { !found } else { found })
        }
        Expr::Compare { col, op, rhs } => {
            let idx = ctx
                .resolve_col(col)
                .ok_or_else(|| DbError::Execution(format!("unknown column '{col}'")))?;
            let lhs = &row[idx];
            if lhs.is_null() || rhs.is_null() {
                return Ok(false);
            }
            let ord = lhs.sql_cmp(rhs);
            let res = match op.as_str() {
                "=" => ord == std::cmp::Ordering::Equal,
                "!=" | "<>" => ord != std::cmp::Ordering::Equal,
                "<" => ord == std::cmp::Ordering::Less,
                "<=" => ord != std::cmp::Ordering::Greater,
                ">" => ord == std::cmp::Ordering::Greater,
                ">=" => ord != std::cmp::Ordering::Less,
                other => {
                    return Err(DbError::Syntax(format!(
                        "unsupported WHERE operator '{other}'"
                    )))
                }
            };
            Ok(res)
        }
    }
}

fn sql_like_match(text: &str, pattern: &str) -> bool {
    let t: Vec<char> = text.to_ascii_lowercase().chars().collect();
    let p: Vec<char> = pattern.to_ascii_lowercase().chars().collect();
    let mut dp = vec![vec![false; p.len() + 1]; t.len() + 1];
    dp[0][0] = true;
    for j in 1..=p.len() {
        if p[j - 1] == '%' {
            dp[0][j] = dp[0][j - 1];
        }
    }
    for i in 1..=t.len() {
        for j in 1..=p.len() {
            if p[j - 1] == '%' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if p[j - 1] == '_' || p[j - 1] == t[i - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }
    dp[t.len()][p.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_binary_sqlite3_header_and_wal_flags() {
        let raw = build_sqlite_header_fixture(4096, 128, true, 14);
        let info = inspect_sqlite_header(&raw).unwrap();
        assert_eq!(info.page_size, 4096);
        assert_eq!(info.page_count, 128);
        assert_eq!(info.write_format, 2);
        assert_eq!(info.journal_mode_hint, "WAL");
        assert_eq!(info.text_encoding, "UTF-8");
        assert_eq!(info.sqlite_version, "3.46.1");
        assert_eq!(info.estimated_db_bytes, 4096 * 128);
        assert_eq!(info.page1_cell_count, Some(14));
    }

    #[test]
    fn parses_sqlite3_65536_page_size_special_encoding() {
        let raw = build_sqlite_header_fixture(65_536, 4, false, 3);
        let info = inspect_sqlite_header(&raw).unwrap();
        assert_eq!(info.page_size, 65_536);
        assert_eq!(info.journal_mode_hint, "Rollback Journal (Legacy)");
    }

    #[test]
    fn rejects_invalid_sqlite3_magic_header() {
        let bad = vec![0u8; 100];
        let err = inspect_sqlite_header(&bad).unwrap_err();
        assert!(matches!(err, DbError::Validation(_)));
    }

    #[test]
    fn seeded_ecommerce_dataset_has_580_rows_across_4_tables() {
        let mut engine = LocalEngine::with_ecommerce_demo();
        let tables = engine.list_tables().unwrap();
        assert_eq!(tables.len(), 4);
        assert_eq!(engine.total_row_count(), 580);
    }

    #[test]
    fn executes_full_crud_join_group_by_order_limit_and_explain() {
        let mut engine = LocalEngine::with_ecommerce_demo();

        // CREATE + INSERT + SELECT + UPDATE + DELETE
        engine
            .execute_query(
                "CREATE TABLE audit_log (id INTEGER PRIMARY KEY, actor TEXT NOT NULL, score INTEGER);",
            )
            .unwrap();
        let ins = engine
            .execute_query(
                "INSERT INTO audit_log (id, actor, score) VALUES (1, 'alice', 90), (2, 'bob', 45), (3, 'carol', 98);",
            )
            .unwrap();
        assert_eq!(ins.affected_rows, 3);

        let sel = engine
            .execute_query(
                "SELECT id, actor, score FROM audit_log WHERE score >= 90 ORDER BY score DESC LIMIT 2;",
            )
            .unwrap();
        assert_eq!(sel.rows.len(), 2);
        assert_eq!(sel.rows[0][1], Value::Text("carol".to_string()));
        assert_eq!(sel.rows[1][1], Value::Text("alice".to_string()));

        let upd = engine
            .execute_query("UPDATE audit_log SET score = 100 WHERE actor = 'bob';")
            .unwrap();
        assert_eq!(upd.affected_rows, 1);

        let del = engine
            .execute_query("DELETE FROM audit_log WHERE id = 1;")
            .unwrap();
        assert_eq!(del.affected_rows, 1);

        // JOIN + GROUP BY + Aggregate on seeded demo tables
        let agg = engine
            .execute_query(
                "SELECT status, COUNT(*) AS order_count, SUM(total_cents) AS revenue FROM orders GROUP BY status ORDER BY order_count DESC;",
            )
            .unwrap();
        assert_eq!(agg.rows.len(), 5);
        assert_eq!(agg.columns[1].name, "order_count");

        let joined = engine
            .execute_query(
                "SELECT o.id, u.email, o.total_cents FROM orders o INNER JOIN users u ON o.user_id = u.id WHERE o.status = 'paid' ORDER BY o.total_cents DESC LIMIT 5;",
            )
            .unwrap();
        assert_eq!(joined.rows.len(), 5);

        let explain = engine
            .execute_query("EXPLAIN SELECT id, email FROM users WHERE active = true ORDER BY id LIMIT 10;")
            .unwrap();
        assert!(explain.rows.len() >= 4);
        assert_eq!(explain.command_tag, "EXPLAIN");
    }
}
