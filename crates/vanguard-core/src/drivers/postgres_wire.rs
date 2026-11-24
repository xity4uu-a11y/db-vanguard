//! Pure-Rust PostgreSQL Frontend/Backend Wire Protocol v3.0 TCP driver.
//!
//! Implements `StartupMessage`, Trust/Cleartext/MD5 authentication, `SimpleQuery ('Q')`,
//! `RowDescription ('T')`, `DataRow ('D')`, `CommandComplete ('C')`, `ErrorResponse ('E')`,
//! `ReadyForQuery ('Z')`, and `Terminate ('X')` with zero C/OpenSSL dependencies.

use crate::drivers::DatabaseDriver;
use crate::exporter::{stream_export_rows, ExportOptions, ExportStats};
use crate::{ColumnMeta, ColumnSchema, ConnectionProfile, DbError, QueryResult, TableSchema, Value};
use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const PG_PROTOCOL_V3: u32 = 0x0003_0000;

/// Raw decoded PostgreSQL backend frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendMessage {
    pub msg_type: u8,
    pub payload: Vec<u8>,
}

/// Decoded PostgreSQL server error payload from an `'E'` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgErrorFields {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
}

/// Pure-Rust PostgreSQL v3 wire-protocol client implementing `DatabaseDriver`.
pub struct PostgresWireDriver {
    stream: Option<TcpStream>,
    server_params: BTreeMap<String, String>,
    backend_pid: i32,
    backend_secret: i32,
    tx_status: u8,
    connect_timeout: Duration,
}

impl Default for PostgresWireDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl PostgresWireDriver {
    /// Creates an unconnected PostgreSQL wire-protocol client.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stream: None,
            server_params: BTreeMap::new(),
            backend_pid: 0,
            backend_secret: 0,
            tx_status: b'I',
            connect_timeout: Duration::from_secs(10),
        }
    }

    /// Returns server parameter values received via `ParameterStatus ('S')` during startup.
    #[must_use]
    pub fn server_parameters(&self) -> &BTreeMap<String, String> {
        &self.server_params
    }

    /// Returns the last transaction status byte from `ReadyForQuery ('Z')` (`b'I'`, `b'T'`, or `b'E'`).
    #[must_use]
    pub fn transaction_status(&self) -> u8 {
        self.tx_status
    }

    /// Gracefully sends `Terminate ('X')` and closes the TCP socket.
    pub fn disconnect(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            let terminate = [b'X', 0, 0, 0, 4];
            let _ = stream.write_all(&terminate);
            let _ = stream.flush();
        }
    }
}

impl Drop for PostgresWireDriver {
    fn drop(&mut self) {
        self.disconnect();
    }
}

impl DatabaseDriver for PostgresWireDriver {
    fn driver_id(&self) -> &'static str {
        "postgres-wire-v3"
    }

    fn connect(&mut self, profile: &ConnectionProfile) -> Result<(), DbError> {
        self.disconnect();
        let host = profile.host.as_deref().unwrap_or("127.0.0.1");
        let port = profile.port.unwrap_or(5432);
        let addr = format!("{host}:{port}");
        let mut stream = TcpStream::connect(&addr)
            .map_err(|e| DbError::Connection(format!("failed to connect to {addr}: {e}")))?;
        let _ = stream.set_read_timeout(Some(self.connect_timeout));
        let _ = stream.set_write_timeout(Some(self.connect_timeout));

        let user = profile.username.as_deref().unwrap_or("postgres");
        let database = if profile.database.is_empty() {
            user
        } else {
            &profile.database
        };
        let password = profile.password.as_deref().unwrap_or("");

        // Send StartupMessage (protocol 3.0)
        let startup = encode_startup_message(user, database, "db-vanguard");
        stream.write_all(&startup)?;
        stream.flush()?;

        // Complete authentication and startup handshake until ReadyForQuery ('Z')
        let mut reader = BufReader::new(stream.try_clone()?);
        loop {
            let msg = read_backend_message(&mut reader)?;
            match msg.msg_type {
                b'R' => {
                    if msg.payload.len() < 4 {
                        return Err(DbError::Protocol(
                            "truncated Authentication message".to_string(),
                        ));
                    }
                    let auth_code = i32::from_be_bytes([
                        msg.payload[0],
                        msg.payload[1],
                        msg.payload[2],
                        msg.payload[3],
                    ]);
                    match auth_code {
                        0 => {} // AuthenticationOk
                        3 => {
                            // AuthenticationCleartextPassword
                            let pw_msg = encode_password_message(password);
                            stream.write_all(&pw_msg)?;
                            stream.flush()?;
                        }
                        5 => {
                            // AuthenticationMD5Password
                            if msg.payload.len() < 8 {
                                return Err(DbError::Protocol(
                                    "missing 4-byte MD5 salt in AuthenticationMD5Password"
                                        .to_string(),
                                ));
                            }
                            let salt = [
                                msg.payload[4],
                                msg.payload[5],
                                msg.payload[6],
                                msg.payload[7],
                            ];
                            let digest = pg_md5_password(user, password, salt);
                            let pw_msg = encode_password_message(&digest);
                            stream.write_all(&pw_msg)?;
                            stream.flush()?;
                        }
                        other => {
                            return Err(DbError::Authentication(format!(
                                "unsupported PostgreSQL authentication method code {other}"
                            )));
                        }
                    }
                }
                b'S' => {
                    if let Some((k, v)) = decode_parameter_status(&msg.payload) {
                        self.server_params.insert(k, v);
                    }
                }
                b'K' => {
                    if msg.payload.len() >= 8 {
                        self.backend_pid = i32::from_be_bytes([
                            msg.payload[0],
                            msg.payload[1],
                            msg.payload[2],
                            msg.payload[3],
                        ]);
                        self.backend_secret = i32::from_be_bytes([
                            msg.payload[4],
                            msg.payload[5],
                            msg.payload[6],
                            msg.payload[7],
                        ]);
                    }
                }
                b'E' => {
                    let err = decode_error_response(&msg.payload);
                    return Err(DbError::Authentication(format!(
                        "[{}] {}: {}",
                        err.code, err.severity, err.message
                    )));
                }
                b'Z' => {
                    self.tx_status = *msg.payload.first().unwrap_or(&b'I');
                    break;
                }
                b'N' => {} // NoticeResponse ignored during startup
                _ => {}
            }
        }

        self.stream = Some(stream);
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn list_tables(&mut self) -> Result<Vec<TableSchema>, DbError> {
        let sql = "SELECT table_schema, table_name FROM information_schema.tables \
                   WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
                   ORDER BY table_schema, table_name;";
        let res = self.execute_query(sql)?;
        let mut tables = Vec::with_capacity(res.rows.len());
        for row in res.rows {
            let schema = row
                .first()
                .map_or_else(|| "public".to_string(), Value::display_string);
            let name = row
                .get(1)
                .map_or_else(String::new, Value::display_string);
            tables.push(TableSchema {
                name,
                schema,
                row_estimate: 0,
                columns: Vec::new(),
            });
        }
        Ok(tables)
    }

    fn describe_table(&mut self, table_name: &str) -> Result<TableSchema, DbError> {
        let safe_name = table_name.replace('\'', "''");
        let sql = format!(
            "SELECT column_name, data_type, is_nullable, column_default \
             FROM information_schema.columns \
             WHERE table_name = '{safe_name}' ORDER BY ordinal_position;"
        );
        let res = self.execute_query(&sql)?;
        if res.rows.is_empty() {
            return Err(DbError::NotFound(format!(
                "table '{table_name}' not found in information_schema"
            )));
        }
        let columns = res
            .rows
            .into_iter()
            .map(|row| ColumnSchema {
                name: row.first().map_or_else(String::new, Value::display_string),
                data_type: row
                    .get(1)
                    .map_or_else(|| "TEXT".to_string(), Value::display_string),
                nullable: row
                    .get(2)
                    .is_some_and(|v| v.display_string().eq_ignore_ascii_case("YES")),
                primary_key: false,
                default_value: row.get(3).and_then(|v| {
                    if v.is_null() {
                        None
                    } else {
                        Some(v.display_string())
                    }
                }),
            })
            .collect();

        Ok(TableSchema {
            name: table_name.to_string(),
            schema: "public".to_string(),
            row_estimate: 0,
            columns,
        })
    }

    fn execute_query(&mut self, sql: &str) -> Result<QueryResult, DbError> {
        let start = Instant::now();
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| DbError::Connection("PostgreSQL client is not connected".to_string()))?;

        let frame = encode_simple_query(sql);
        stream.write_all(&frame)?;
        stream.flush()?;

        let mut reader = BufReader::new(stream.try_clone()?);
        let mut columns: Vec<ColumnMeta> = Vec::new();
        let mut col_oids: Vec<u32> = Vec::new();
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut command_tag = String::new();
        let mut pending_error: Option<DbError> = None;

        loop {
            let msg = read_backend_message(&mut reader)?;
            match msg.msg_type {
                b'T' => {
                    let (metas, oids) = decode_row_description(&msg.payload)?;
                    columns = metas;
                    col_oids = oids;
                }
                b'D' => {
                    let row = decode_data_row(&msg.payload, &col_oids)?;
                    rows.push(row);
                }
                b'C' => {
                    command_tag = read_cstr(&msg.payload, 0).0;
                }
                b'I' => {
                    command_tag = "EMPTY".to_string();
                }
                b'E' => {
                    let err = decode_error_response(&msg.payload);
                    pending_error = Some(DbError::Execution(format!(
                        "[{}] {}: {}",
                        err.code, err.severity, err.message
                    )));
                }
                b'Z' => {
                    self.tx_status = *msg.payload.first().unwrap_or(&b'I');
                    break;
                }
                b'N' | b'S' => {}
                _ => {}
            }
        }

        if let Some(err) = pending_error {
            return Err(err);
        }

        let affected_rows = parse_command_tag_count(&command_tag).unwrap_or(rows.len() as u64);
        Ok(QueryResult {
            columns,
            rows,
            affected_rows,
            execution_ms: start.elapsed().as_secs_f64() * 1000.0,
            command_tag,
        })
    }

    fn stream_export(
        &mut self,
        sql: &str,
        options: &ExportOptions,
        writer: &mut dyn Write,
    ) -> Result<ExportStats, DbError> {
        let stream = self
            .stream
            .as_mut()
            .ok_or_else(|| DbError::Connection("PostgreSQL client is not connected".to_string()))?;

        let frame = encode_simple_query(sql);
        stream.write_all(&frame)?;
        stream.flush()?;

        let mut reader = BufReader::new(stream.try_clone()?);
        let mut columns: Vec<ColumnMeta> = Vec::new();
        let mut col_oids: Vec<u32> = Vec::new();
        let mut ready_seen = false;
        let mut pending_error: Option<DbError> = None;

        // Read ahead until RowDescription ('T') or terminal ReadyForQuery ('Z')
        let mut first_row: Option<Vec<Value>> = None;
        while !ready_seen {
            let msg = read_backend_message(&mut reader)?;
            match msg.msg_type {
                b'T' => {
                    let (metas, oids) = decode_row_description(&msg.payload)?;
                    columns = metas;
                    col_oids = oids;
                    break;
                }
                b'D' => {
                    first_row = Some(decode_data_row(&msg.payload, &col_oids)?);
                    break;
                }
                b'E' => {
                    let err = decode_error_response(&msg.payload);
                    pending_error = Some(DbError::Execution(format!(
                        "[{}] {}: {}",
                        err.code, err.severity, err.message
                    )));
                }
                b'Z' => {
                    self.tx_status = *msg.payload.first().unwrap_or(&b'I');
                    ready_seen = true;
                }
                _ => {}
            }
        }

        if let Some(err) = pending_error {
            return Err(err);
        }

        let mut stream_err: Option<DbError> = None;
        let mut tx_status = self.tx_status;

        let stats = {
            let row_iter = PgRowIterator {
                reader: &mut reader,
                col_oids: &col_oids,
                buffered_first: first_row.take(),
                done: ready_seen,
                error: &mut stream_err,
                tx_status: &mut tx_status,
            };
            stream_export_rows(&columns, row_iter, options, writer)?
        };

        self.tx_status = tx_status;
        if let Some(err) = stream_err {
            return Err(err);
        }

        Ok(stats)
    }
}

struct PgRowIterator<'a, R: Read> {
    reader: &'a mut BufReader<R>,
    col_oids: &'a [u32],
    buffered_first: Option<Vec<Value>>,
    done: bool,
    error: &'a mut Option<DbError>,
    tx_status: &'a mut u8,
}

impl<R: Read> Iterator for PgRowIterator<'_, R> {
    type Item = Vec<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(row) = self.buffered_first.take() {
            return Some(row);
        }
        if self.done {
            return None;
        }
        loop {
            match read_backend_message(self.reader) {
                Ok(msg) => match msg.msg_type {
                    b'D' => match decode_data_row(&msg.payload, self.col_oids) {
                        Ok(row) => return Some(row),
                        Err(e) => {
                            *self.error = Some(e);
                            self.done = true;
                            return None;
                        }
                    },
                    b'E' => {
                        let err = decode_error_response(&msg.payload);
                        *self.error = Some(DbError::Execution(format!(
                            "[{}] {}: {}",
                            err.code, err.severity, err.message
                        )));
                    }
                    b'Z' => {
                        *self.tx_status = *msg.payload.first().unwrap_or(&b'I');
                        self.done = true;
                        return None;
                    }
                    _ => {}
                },
                Err(e) => {
                    *self.error = Some(e);
                    self.done = true;
                    return None;
                }
            }
        }
    }
}

// --- PostgreSQL v3 Wire Protocol Encoders & Decoders ---

/// Encodes a PostgreSQL v3 `StartupMessage` frame.
#[must_use]
pub fn encode_startup_message(user: &str, database: &str, app_name: &str) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&PG_PROTOCOL_V3.to_be_bytes());
    push_kv(&mut body, "user", user);
    push_kv(&mut body, "database", database);
    push_kv(&mut body, "application_name", app_name);
    body.push(0); // terminating NUL

    let total_len = (body.len() + 4) as i32;
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&total_len.to_be_bytes());
    frame.extend_from_slice(&body);
    frame
}

fn push_kv(buf: &mut Vec<u8>, key: &str, val: &str) {
    buf.extend_from_slice(key.as_bytes());
    buf.push(0);
    buf.extend_from_slice(val.as_bytes());
    buf.push(0);
}

/// Encodes a `'p'` `PasswordMessage` frame.
#[must_use]
pub fn encode_password_message(password: &str) -> Vec<u8> {
    let len = (4 + password.len() + 1) as i32;
    let mut frame = Vec::with_capacity(1 + len as usize);
    frame.push(b'p');
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(password.as_bytes());
    frame.push(0);
    frame
}

/// Encodes a `'Q'` `SimpleQuery` frame.
#[must_use]
pub fn encode_simple_query(sql: &str) -> Vec<u8> {
    let len = (4 + sql.len() + 1) as i32;
    let mut frame = Vec::with_capacity(1 + len as usize);
    frame.push(b'Q');
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(sql.as_bytes());
    frame.push(0);
    frame
}

/// Reads a single framed backend message `[type: u8][len: i32][payload: len-4]`.
pub fn read_backend_message<R: Read>(reader: &mut R) -> Result<BackendMessage, DbError> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header)?;
    let msg_type = header[0];
    let length = i32::from_be_bytes([header[1], header[2], header[3], header[4]]);
    if length < 4 {
        return Err(DbError::Protocol(format!(
            "invalid PostgreSQL backend message length: {length}"
        )));
    }
    let payload_len = (length - 4) as usize;
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload)?;
    Ok(BackendMessage { msg_type, payload })
}

/// Decodes a `'T'` `RowDescription` payload into column metadata and PostgreSQL type OIDs.
pub fn decode_row_description(payload: &[u8]) -> Result<(Vec<ColumnMeta>, Vec<u32>), DbError> {
    if payload.len() < 2 {
        return Err(DbError::Protocol(
            "truncated RowDescription payload".to_string(),
        ));
    }
    let field_count = i16::from_be_bytes([payload[0], payload[1]]) as usize;
    let mut offset = 2usize;
    let mut metas = Vec::with_capacity(field_count);
    let mut oids = Vec::with_capacity(field_count);

    for _ in 0..field_count {
        let (name, next_off) = read_cstr(payload, offset);
        offset = next_off;
        if offset + 18 > payload.len() {
            return Err(DbError::Protocol(
                "truncated field descriptor in RowDescription".to_string(),
            ));
        }
        // table_oid (4) + attr_num (2) + type_oid (4) + type_size (2) + type_mod (4) + format (2)
        let type_oid = u32::from_be_bytes([
            payload[offset + 6],
            payload[offset + 7],
            payload[offset + 8],
            payload[offset + 9],
        ]);
        offset += 18;
        metas.push(ColumnMeta {
            name,
            data_type: pg_oid_to_type_name(type_oid).to_string(),
            nullable: true,
        });
        oids.push(type_oid);
    }

    Ok((metas, oids))
}

/// Decodes a `'D'` `DataRow` payload into typed `Value` cells using PostgreSQL column OIDs.
pub fn decode_data_row(payload: &[u8], col_oids: &[u32]) -> Result<Vec<Value>, DbError> {
    if payload.len() < 2 {
        return Err(DbError::Protocol("truncated DataRow payload".to_string()));
    }
    let col_count = i16::from_be_bytes([payload[0], payload[1]]) as usize;
    let mut offset = 2usize;
    let mut values = Vec::with_capacity(col_count);

    for idx in 0..col_count {
        if offset + 4 > payload.len() {
            return Err(DbError::Protocol("truncated column length in DataRow".to_string()));
        }
        let col_len = i32::from_be_bytes([
            payload[offset],
            payload[offset + 1],
            payload[offset + 2],
            payload[offset + 3],
        ]);
        offset += 4;
        if col_len == -1 {
            values.push(Value::Null);
        } else if col_len < 0 {
            return Err(DbError::Protocol(format!(
                "invalid negative column length {col_len} in DataRow"
            )));
        } else {
            let len = col_len as usize;
            if offset + len > payload.len() {
                return Err(DbError::Protocol("truncated column bytes in DataRow".to_string()));
            }
            let raw = String::from_utf8_lossy(&payload[offset..offset + len]).into_owned();
            offset += len;
            let oid = col_oids.get(idx).copied().unwrap_or(25);
            values.push(parse_pg_text_value(&raw, oid));
        }
    }

    Ok(values)
}

fn parse_pg_text_value(raw: &str, oid: u32) -> Value {
    match oid {
        16 => match raw {
            "t" | "true" | "TRUE" | "1" => Value::Bool(true),
            "f" | "false" | "FALSE" | "0" => Value::Bool(false),
            _ => Value::Text(raw.to_string()),
        },
        20 | 21 | 23 => raw
            .parse::<i64>()
            .map_or_else(|_| Value::Text(raw.to_string()), Value::Integer),
        700 | 701 | 1700 => raw
            .parse::<f64>()
            .map_or_else(|_| Value::Text(raw.to_string()), Value::Float),
        _ => Value::Text(raw.to_string()),
    }
}

/// Maps well-known PostgreSQL type OIDs to canonical SQL type names.
#[must_use]
pub fn pg_oid_to_type_name(oid: u32) -> &'static str {
    match oid {
        16 => "BOOLEAN",
        20 => "BIGINT",
        21 => "SMALLINT",
        23 => "INTEGER",
        25 => "TEXT",
        114 => "JSON",
        700 => "REAL",
        701 => "DOUBLE PRECISION",
        1043 => "VARCHAR",
        1082 => "DATE",
        1114 => "TIMESTAMP",
        1184 => "TIMESTAMPTZ",
        1700 => "NUMERIC",
        2950 => "UUID",
        3802 => "JSONB",
        _ => "TEXT",
    }
}

/// Decodes a `'S'` `ParameterStatus` key/value pair.
fn decode_parameter_status(payload: &[u8]) -> Option<(String, String)> {
    let (key, next) = read_cstr(payload, 0);
    if key.is_empty() || next > payload.len() {
        return None;
    }
    let (val, _) = read_cstr(payload, next);
    Some((key, val))
}

/// Decodes an `'E'` `ErrorResponse` payload into structured fields.
#[must_use]
pub fn decode_error_response(payload: &[u8]) -> PgErrorFields {
    let mut severity = "ERROR".to_string();
    let mut code = "00000".to_string();
    let mut message = "PostgreSQL server error".to_string();
    let mut detail = None;
    let mut hint = None;

    let mut offset = 0usize;
    while offset < payload.len() {
        let field_type = payload[offset];
        offset += 1;
        if field_type == 0 {
            break;
        }
        let (val, next) = read_cstr(payload, offset);
        offset = next;
        match field_type {
            b'S' | b'V' => severity = val,
            b'C' => code = val,
            b'M' => message = val,
            b'D' => detail = Some(val),
            b'H' => hint = Some(val),
            _ => {}
        }
    }

    PgErrorFields {
        severity,
        code,
        message,
        detail,
        hint,
    }
}

fn read_cstr(buf: &[u8], start: usize) -> (String, usize) {
    if start >= buf.len() {
        return (String::new(), buf.len());
    }
    let end = buf[start..]
        .iter()
        .position(|&b| b == 0)
        .map_or(buf.len(), |rel| start + rel);
    let text = String::from_utf8_lossy(&buf[start..end]).into_owned();
    let next = if end < buf.len() { end + 1 } else { buf.len() };
    (text, next)
}

fn parse_command_tag_count(tag: &str) -> Option<u64> {
    tag.split_whitespace().last()?.parse::<u64>().ok()
}

/// Computes PostgreSQL MD5 authentication token: `"md5" + hex(md5(hex(md5(password + user)) + salt))`.
#[must_use]
pub fn pg_md5_password(user: &str, password: &str, salt: [u8; 4]) -> String {
    let mut inner_input = Vec::with_capacity(password.len() + user.len());
    inner_input.extend_from_slice(password.as_bytes());
    inner_input.extend_from_slice(user.as_bytes());
    let inner_hex = md5_hex(&inner_input);

    let mut outer_input = Vec::with_capacity(inner_hex.len() + 4);
    outer_input.extend_from_slice(inner_hex.as_bytes());
    outer_input.extend_from_slice(&salt);
    format!("md5{}", md5_hex(&outer_input))
}

/// Pure-Rust RFC 1321 MD5 implementation (used exclusively for PostgreSQL wire auth challenge).
fn md5_hex(data: &[u8]) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6,
        10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76a_a478, 0xe8c7_b756, 0x2420_70db, 0xc1bd_ceee, 0xf57c_0faf, 0x4787_c62a, 0xa830_4613,
        0xfd46_9501, 0x6980_98d8, 0x8b44_f7af, 0xffff_5bb1, 0x895c_d7be, 0x6b90_1122, 0xfd98_7193,
        0xa679_438e, 0x49b4_0821, 0xf61e_2562, 0xc040_b340, 0x265e_5a51, 0xe9b6_c7aa, 0xd62f_105d,
        0x0244_1453, 0xd8a1_e681, 0xe7d3_fbc8, 0x21e1_cde6, 0xc337_07d6, 0xf4d5_0d87, 0x455a_14ed,
        0xa9e3_e905, 0xfcef_a3f8, 0x676f_02d9, 0x8d2a_4c8a, 0xfffa_3942, 0x8771_f681, 0x6d9d_6122,
        0xfde5_380c, 0xa4be_ea44, 0x4bde_cfa9, 0xf6bb_4b60, 0xbebf_bc70, 0x289b_7ec6, 0xeaa1_27fa,
        0xd4ef_3085, 0x0488_1d05, 0xd9d4_d039, 0xe6db_99e5, 0x1fa2_7cf8, 0xc4ac_5665, 0xf429_2244,
        0x432a_ff97, 0xab94_23a7, 0xfc93_a039, 0x655b_59c3, 0x8f0c_cc92, 0xffef_f47d, 0x8584_5dd1,
        0x6fa8_7e4f, 0xfe2c_e6e0, 0xa301_4314, 0x4e08_11a1, 0xf753_7e82, 0xbd3a_f235, 0x2ad7_d2bb,
        0xeb86_d391,
    ];

    let mut msg = data.to_vec();
    let orig_bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&orig_bit_len.to_le_bytes());

    let mut a0: u32 = 0x6745_2301;
    let mut b0: u32 = 0xefcd_ab89;
    let mut c0: u32 = 0x98ba_dcfe;
    let mut d0: u32 = 0x1032_5476;

    for chunk in msg.as_chunks::<64>().0 {
        let mut m = [0u32; 16];
        for (i, word) in chunk.as_chunks::<4>().0.iter().enumerate() {
            m[i] = u32::from_le_bytes(*word);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            let sum = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
            b = b.wrapping_add(sum.rotate_left(S[i]));
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut digest = [0u8; 16];
    digest[0..4].copy_from_slice(&a0.to_le_bytes());
    digest[4..8].copy_from_slice(&b0.to_le_bytes());
    digest[8..12].copy_from_slice(&c0.to_le_bytes());
    digest[12..16].copy_from_slice(&d0.to_le_bytes());

    let mut out = String::with_capacity(32);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exporter::ExportFormat;
    use std::net::TcpListener;
    use std::thread;

    fn send_msg(stream: &mut TcpStream, msg_type: u8, payload: &[u8]) {
        let len = (payload.len() + 4) as i32;
        stream.write_all(&[msg_type]).unwrap();
        stream.write_all(&len.to_be_bytes()).unwrap();
        stream.write_all(payload).unwrap();
    }

    fn build_row_desc(cols: &[(&str, u32)]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&(cols.len() as i16).to_be_bytes());
        for (idx, (name, oid)) in cols.iter().enumerate() {
            p.extend_from_slice(name.as_bytes());
            p.push(0);
            p.extend_from_slice(&0i32.to_be_bytes()); // table oid
            p.extend_from_slice(&((idx + 1) as i16).to_be_bytes()); // attr num
            p.extend_from_slice(&oid.to_be_bytes()); // type oid
            p.extend_from_slice(&(-1i16).to_be_bytes()); // type size
            p.extend_from_slice(&(-1i32).to_be_bytes()); // type mod
            p.extend_from_slice(&0i16.to_be_bytes()); // text format
        }
        p
    }

    fn build_data_row(cells: &[Option<&str>]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&(cells.len() as i16).to_be_bytes());
        for cell in cells {
            match cell {
                None => p.extend_from_slice(&(-1i32).to_be_bytes()),
                Some(s) => {
                    p.extend_from_slice(&(s.len() as i32).to_be_bytes());
                    p.extend_from_slice(s.as_bytes());
                }
            }
        }
        p
    }

    fn read_startup(stream: &mut TcpStream) {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).unwrap();
        let total = i32::from_be_bytes(len_buf) as usize;
        let mut body = vec![0u8; total - 4];
        stream.read_exact(&mut body).unwrap();
        assert_eq!(&body[0..4], &PG_PROTOCOL_V3.to_be_bytes());
    }

    fn read_frontend_msg(stream: &mut TcpStream) -> (u8, Vec<u8>) {
        let mut hdr = [0u8; 5];
        stream.read_exact(&mut hdr).unwrap();
        let len = i32::from_be_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
        let mut payload = vec![0u8; len - 4];
        stream.read_exact(&mut payload).unwrap();
        (hdr[0], payload)
    }

    #[test]
    fn postgres_wire_client_executes_query_and_streams_export_against_mock_tcp_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            read_startup(&mut sock);

            // Request cleartext auth
            send_msg(&mut sock, b'R', &3i32.to_be_bytes());
            let (pw_type, pw_payload) = read_frontend_msg(&mut sock);
            assert_eq!(pw_type, b'p');
            assert_eq!(read_cstr(&pw_payload, 0).0, "mock-pg-pass");

            // Send AuthOk, ParameterStatus, BackendKeyData, ReadyForQuery
            send_msg(&mut sock, b'R', &0i32.to_be_bytes());
            send_msg(&mut sock, b'S', b"server_version\x0016.4\0");
            let mut key_data = Vec::new();
            key_data.extend_from_slice(&4242i32.to_be_bytes());
            key_data.extend_from_slice(&9999i32.to_be_bytes());
            send_msg(&mut sock, b'K', &key_data);
            send_msg(&mut sock, b'Z', b"I");

            // Query 1: normal SELECT returning typed columns & NULL
            let (q1_type, q1_payload) = read_frontend_msg(&mut sock);
            assert_eq!(q1_type, b'Q');
            assert!(read_cstr(&q1_payload, 0).0.contains("SELECT id"));

            let row_desc = build_row_desc(&[
                ("id", 23),
                ("email", 25),
                ("active", 16),
                ("balance", 701),
                ("note", 25),
            ]);
            send_msg(&mut sock, b'T', &row_desc);
            send_msg(
                &mut sock,
                b'D',
                &build_data_row(&[
                    Some("101"),
                    Some("ada@example.org"),
                    Some("t"),
                    Some("42.75"),
                    None,
                ]),
            );
            send_msg(
                &mut sock,
                b'D',
                &build_data_row(&[
                    Some("102"),
                    Some("linus@example.org"),
                    Some("f"),
                    Some("0.0"),
                    Some("=SUM(A1:A2)"),
                ]),
            );
            send_msg(&mut sock, b'C', b"SELECT 2\0");
            send_msg(&mut sock, b'Z', b"I");

            // Query 2: stream_export over wire
            let (q2_type, _) = read_frontend_msg(&mut sock);
            assert_eq!(q2_type, b'Q');
            send_msg(&mut sock, b'T', &row_desc);
            send_msg(
                &mut sock,
                b'D',
                &build_data_row(&[
                    Some("102"),
                    Some("linus@example.org"),
                    Some("f"),
                    Some("0.0"),
                    Some("=SUM(A1:A2)"),
                ]),
            );
            send_msg(&mut sock, b'C', b"SELECT 1\0");
            send_msg(&mut sock, b'Z', b"I");

            // Query 3: ErrorResponse ('E')
            let (q3_type, _) = read_frontend_msg(&mut sock);
            assert_eq!(q3_type, b'Q');
            send_msg(
                &mut sock,
                b'E',
                b"SERROR\0C42P01\0Mrelation \"missing_tbl\" does not exist\0\0",
            );
            send_msg(&mut sock, b'Z', b"I");

            // Expect Terminate ('X')
            let (term_type, _) = read_frontend_msg(&mut sock);
            assert_eq!(term_type, b'X');
        });

        let mut driver = PostgresWireDriver::new();
        let profile = ConnectionProfile {
            id: "pg-test".to_string(),
            name: "Mock PG".to_string(),
            driver: "postgres-wire-v3".to_string(),
            host: Some("127.0.0.1".to_string()),
            port: Some(port),
            database: "vanguard_db".to_string(),
            username: Some("vanguard".to_string()),
            password: Some("mock-pg-pass".to_string()),
            read_only: false,
        };

        driver.connect(&profile).unwrap();
        assert!(driver.is_connected());
        assert_eq!(
            driver.server_parameters().get("server_version").map(String::as_str),
            Some("16.4")
        );

        let result = driver
            .execute_query("SELECT id, email, active, balance, note FROM users;")
            .unwrap();
        assert_eq!(result.columns.len(), 5);
        assert_eq!(result.columns[0].data_type, "INTEGER");
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], Value::Integer(101));
        assert_eq!(result.rows[0][2], Value::Bool(true));
        assert_eq!(result.rows[0][4], Value::Null);
        assert_eq!(result.affected_rows, 2);

        // Wire-to-CSV streaming export with OWASP formula neutralization
        let mut csv_buf = Vec::new();
        let stats = driver
            .stream_export(
                "SELECT id, email, active, balance, note FROM users LIMIT 1;",
                &ExportOptions {
                    format: ExportFormat::Csv,
                    ..ExportOptions::default()
                },
                &mut csv_buf,
            )
            .unwrap();
        assert_eq!(stats.rows_exported, 1);
        let csv_str = String::from_utf8(csv_buf).unwrap();
        assert!(csv_str.contains("'=SUM(A1:A2)"));

        // Structured ErrorResponse handling
        let err = driver
            .execute_query("SELECT * FROM missing_tbl;")
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("42P01"));
        assert!(msg.contains("missing_tbl"));

        driver.disconnect();
        server.join().unwrap();
    }

    #[test]
    fn computes_rfc1321_postgres_md5_auth_token_accurately() {
        // Known reference vector: md5("postgres") = "e8a48653851e28c69d0506508fb27fc5"
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        let token = pg_md5_password("postgres", "pass", [0x01, 0x02, 0x03, 0x04]);
        assert!(token.starts_with("md5"));
        assert_eq!(token.len(), 35);
    }
}
