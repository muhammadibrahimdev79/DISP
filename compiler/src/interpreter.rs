use crate::ast::{
    AssignmentOperator, BinaryOperator, Block, EnumDeclaration, Expr, Expression, Function,
    Pattern, Program, Statement, TypeName, TypeQualifier, UnaryOperator, VariantDeclaration,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use native_tls::{Protocol, TlsConnector, TlsStream as NativeTlsStream};
use std::{
    any::Any,
    collections::{HashMap, VecDeque},
    fs,
    io::{self, Read, Write},
    net::{
        IpAddr, Shutdown, TcpListener as StdTcpListener, TcpStream as StdTcpStream, ToSocketAddrs,
        UdpSocket as StdUdpSocket,
    },
    path::PathBuf,
    process::{Child as StdChild, ChildStdin, Command as StdCommand, Stdio},
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    thread,
    time::{Duration as StdDuration, Instant as StdInstant, SystemTime, UNIX_EPOCH},
};
use url::{Host, Position, Url};

#[repr(C)]
struct Sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
struct SqliteStatement {
    _private: [u8; 0],
}

type SqliteCallback = unsafe extern "C" fn(
    context: *mut std::ffi::c_void,
    columns: std::ffi::c_int,
    values: *mut *mut std::ffi::c_char,
    names: *mut *mut std::ffi::c_char,
) -> std::ffi::c_int;

#[cfg_attr(windows, link(name = "winsqlite3"))]
#[cfg_attr(not(windows), link(name = "sqlite3"))]
unsafe extern "C" {
    fn sqlite3_open_v2(
        filename: *const std::ffi::c_char,
        database: *mut *mut Sqlite3,
        flags: std::ffi::c_int,
        vfs: *const std::ffi::c_char,
    ) -> std::ffi::c_int;
    fn sqlite3_close_v2(database: *mut Sqlite3) -> std::ffi::c_int;
    fn sqlite3_errmsg(database: *mut Sqlite3) -> *const std::ffi::c_char;
    fn sqlite3_busy_timeout(database: *mut Sqlite3, millis: std::ffi::c_int) -> std::ffi::c_int;
    fn sqlite3_prepare_v2(
        database: *mut Sqlite3,
        sql: *const std::ffi::c_char,
        bytes: std::ffi::c_int,
        statement: *mut *mut SqliteStatement,
        tail: *mut *const std::ffi::c_char,
    ) -> std::ffi::c_int;
    fn sqlite3_finalize(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_step(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_bind_parameter_count(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_bind_null(
        statement: *mut SqliteStatement,
        index: std::ffi::c_int,
    ) -> std::ffi::c_int;
    fn sqlite3_bind_int64(
        statement: *mut SqliteStatement,
        index: std::ffi::c_int,
        value: i64,
    ) -> std::ffi::c_int;
    fn sqlite3_bind_double(
        statement: *mut SqliteStatement,
        index: std::ffi::c_int,
        value: f64,
    ) -> std::ffi::c_int;
    fn sqlite3_bind_text(
        statement: *mut SqliteStatement,
        index: std::ffi::c_int,
        value: *const std::ffi::c_char,
        bytes: std::ffi::c_int,
        destructor: Option<unsafe extern "C" fn(*mut std::ffi::c_void)>,
    ) -> std::ffi::c_int;
    fn sqlite3_column_count(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_column_name(
        statement: *mut SqliteStatement,
        column: std::ffi::c_int,
    ) -> *const std::ffi::c_char;
    fn sqlite3_column_type(
        statement: *mut SqliteStatement,
        column: std::ffi::c_int,
    ) -> std::ffi::c_int;
    fn sqlite3_column_int64(statement: *mut SqliteStatement, column: std::ffi::c_int) -> i64;
    fn sqlite3_column_double(statement: *mut SqliteStatement, column: std::ffi::c_int) -> f64;
    fn sqlite3_column_text(statement: *mut SqliteStatement, column: std::ffi::c_int) -> *const u8;
    fn sqlite3_column_bytes(
        statement: *mut SqliteStatement,
        column: std::ffi::c_int,
    ) -> std::ffi::c_int;
    fn sqlite3_changes(database: *mut Sqlite3) -> std::ffi::c_int;
    fn sqlite3_last_insert_rowid(database: *mut Sqlite3) -> i64;
    fn sqlite3_get_autocommit(database: *mut Sqlite3) -> std::ffi::c_int;
    fn sqlite3_exec(
        database: *mut Sqlite3,
        sql: *const std::ffi::c_char,
        callback: Option<SqliteCallback>,
        context: *mut std::ffi::c_void,
        error: *mut *mut std::ffi::c_char,
    ) -> std::ffi::c_int;
}

#[derive(Clone)]
struct RuntimeThread(Arc<ThreadState>);

type ThreadResult = Result<Box<dyn Any + Send>, Diagnostic>;
type ThreadHandle = thread::JoinHandle<ThreadResult>;

struct ThreadState {
    handle: StdMutex<Option<ThreadHandle>>,
}

impl RuntimeThread {
    fn new(handle: ThreadHandle) -> Self {
        Self(Arc::new(ThreadState {
            handle: StdMutex::new(Some(handle)),
        }))
    }

    fn join(&self, span: Span) -> Result<Value, Diagnostic> {
        let handle = self
            .0
            .handle
            .lock()
            .map_err(|_| {
                Diagnostic::new(DiagnosticKind::Runtime, "thread state is poisoned", span)
            })?
            .take()
            .ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Runtime,
                    "thread has already been joined",
                    span,
                )
            })?;
        let value = handle.join().map_err(|_| {
            Diagnostic::new(DiagnosticKind::Runtime, "spawned thread panicked", span)
        })??;
        value.downcast::<Value>().map(|value| *value).map_err(|_| {
            Diagnostic::new(
                DiagnosticKind::Runtime,
                "thread returned an invalid value",
                span,
            )
        })
    }
}

impl Drop for ThreadState {
    fn drop(&mut self) {
        if let Ok(slot) = self.handle.get_mut()
            && let Some(handle) = slot.take()
        {
            let _ = handle.join();
        }
    }
}

impl std::fmt::Debug for RuntimeThread {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Thread")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeThread {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeFuture(Arc<StdMutex<Option<FutureWork>>>);

enum FutureWork {
    Function(Box<Function>, Vec<Value>),
    Yield,
    Sleep(StdDuration),
    ReadText(PathBuf),
    ReadBytes(PathBuf),
    WriteText(PathBuf, RuntimeString),
    WriteBytes(PathBuf, Vec<u8>),
    Connect(RuntimeSocketAddress, Option<StdDuration>),
    Accept(RuntimeTcpListener, Option<StdDuration>),
    SocketRead(RuntimeTcpStream, usize, Option<StdDuration>),
    SocketWrite(RuntimeTcpStream, Vec<u8>, Option<StdDuration>),
    UdpReceive(RuntimeUdpSocket, usize, Option<StdDuration>),
    UdpSend(
        RuntimeUdpSocket,
        Vec<u8>,
        RuntimeSocketAddress,
        Option<StdDuration>,
    ),
    Resolve(String, Option<StdDuration>),
    TlsConnect(RuntimeTcpStream, String, Option<StdDuration>),
    TlsRead(RuntimeTlsStream, usize, Option<StdDuration>),
    TlsWrite(RuntimeTlsStream, Vec<u8>, Option<StdDuration>),
    HttpRequest(RuntimeHttpRequest, StdDuration),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeSocketAddress {
    host: String,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeIpAddress(IpAddr);

#[derive(Clone)]
struct RuntimeTcpStream(Arc<StdMutex<RuntimeTcpStreamState>>);

#[derive(Clone)]
struct RuntimeTlsStream(Arc<StdMutex<Option<NativeTlsStream<StdTcpStream>>>>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeHttpResponse(Arc<RuntimeHttpResponseData>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeHttpRequest {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct RuntimeHttpResponseData {
    status: u16,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeJson {
    text: String,
    kind: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeUrl {
    text: String,
}

const HTTP_URL_LIMIT: usize = 8192;

impl RuntimeUrl {
    fn scheme(&self) -> &str {
        self.text.split_once(':').map_or("", |(scheme, _)| scheme)
    }

    fn authority(&self) -> &str {
        let start = self.text.find("://").map_or(0, |index| index + 3);
        let tail = &self.text[start..];
        let end = tail.find(['/', '?']).unwrap_or(tail.len());
        &tail[..end]
    }

    fn host(&self) -> Option<&str> {
        let authority = self.authority();
        if let Some(bracketed) = authority.strip_prefix('[') {
            return bracketed.split_once(']').map(|(host, _)| host);
        }
        Some(
            authority
                .rsplit_once(':')
                .map_or(authority, |(host, _)| host),
        )
        .filter(|host| !host.is_empty())
    }

    fn port(&self) -> Option<u16> {
        let authority = self.authority();
        let port = if authority.starts_with('[') {
            authority.split_once("]:").map(|(_, port)| port)
        } else {
            authority.rsplit_once(':').map(|(_, port)| port)
        }?;
        port.parse().ok()
    }

    fn path(&self) -> &str {
        let start = self.text.find("://").map_or(0, |index| index + 3);
        let tail = &self.text[start..];
        let Some(path) = tail.find('/') else {
            return "/";
        };
        let path = &tail[path..];
        path.split_once('?').map_or(path, |(path, _)| path)
    }

    fn query(&self) -> Option<&str> {
        self.text.split_once('?').map(|(_, query)| query)
    }

    fn encoded_component_len(value: &str) -> Option<usize> {
        value.bytes().try_fold(0usize, |length, byte| {
            length.checked_add(
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                    1
                } else {
                    3
                },
            )
        })
    }

    fn encoded_component(value: &str, length: usize) -> String {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut encoded = String::with_capacity(length);
        for byte in value.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                encoded.push(char::from(byte));
            } else {
                encoded.push('%');
                encoded.push(char::from(HEX[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX[usize::from(byte & 15)]));
            }
        }
        encoded
    }

    fn join_path(&self, segment: &str) -> io::Result<Self> {
        if segment.is_empty() || matches!(segment, "." | "..") {
            return Err(http_error(
                io::ErrorKind::InvalidInput,
                "URL path segment must be non-empty and cannot be '.' or '..'",
            ));
        }
        let (base, query) = self
            .text
            .split_once('?')
            .map_or((self.text.as_str(), None), |(base, query)| {
                (base, Some(query))
            });
        let encoded_len = Self::encoded_component_len(segment)
            .ok_or_else(|| http_error(io::ErrorKind::InvalidInput, "URL size overflow"))?;
        let needed = self
            .text
            .len()
            .checked_add(usize::from(!base.ends_with('/')))
            .and_then(|length| length.checked_add(encoded_len))
            .ok_or_else(|| http_error(io::ErrorKind::InvalidInput, "URL size overflow"))?;
        if needed > HTTP_URL_LIMIT {
            return Err(http_error(
                io::ErrorKind::InvalidInput,
                "URL exceeds the 8192-byte safety limit",
            ));
        }
        let encoded = Self::encoded_component(segment, encoded_len);
        let mut text = String::with_capacity(needed);
        text.push_str(base);
        if !base.ends_with('/') {
            text.push('/');
        }
        text.push_str(&encoded);
        if let Some(query) = query {
            text.push('?');
            text.push_str(query);
        }
        parse_http_url(&text)?;
        Ok(Self { text })
    }

    fn query_param(&self, name: &str, value: &str) -> io::Result<Self> {
        if name.is_empty() {
            return Err(http_error(
                io::ErrorKind::InvalidInput,
                "URL query parameter name must not be empty",
            ));
        }
        let has_query = self.text.contains('?');
        let separator = usize::from(!has_query || !self.text.ends_with(['?', '&']));
        let name_len = Self::encoded_component_len(name)
            .ok_or_else(|| http_error(io::ErrorKind::InvalidInput, "URL size overflow"))?;
        let value_len = Self::encoded_component_len(value)
            .ok_or_else(|| http_error(io::ErrorKind::InvalidInput, "URL size overflow"))?;
        let needed = self
            .text
            .len()
            .checked_add(separator)
            .and_then(|length| length.checked_add(name_len))
            .and_then(|length| length.checked_add(1))
            .and_then(|length| length.checked_add(value_len))
            .ok_or_else(|| http_error(io::ErrorKind::InvalidInput, "URL size overflow"))?;
        if needed > HTTP_URL_LIMIT {
            return Err(http_error(
                io::ErrorKind::InvalidInput,
                "URL exceeds the 8192-byte safety limit",
            ));
        }
        let mut text = String::with_capacity(needed);
        text.push_str(&self.text);
        if !has_query {
            text.push('?');
        } else if !text.ends_with(['?', '&']) {
            text.push('&');
        }
        text.push_str(&Self::encoded_component(name, name_len));
        text.push('=');
        text.push_str(&Self::encoded_component(value, value_len));
        parse_http_url(&text)?;
        Ok(Self { text })
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl JsonParser<'_> {
    fn space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.at += 1;
        }
    }

    fn string(&mut self) -> Result<(), &'static str> {
        if self.bytes.get(self.at) != Some(&b'"') {
            return Err("JSON object key must be a string");
        }
        self.at += 1;
        while let Some(&byte) = self.bytes.get(self.at) {
            self.at += 1;
            match byte {
                b'"' => return Ok(()),
                0..=0x1f => return Err("JSON string contains a control character"),
                b'\\' => {
                    let escape = *self.bytes.get(self.at).ok_or("JSON escape is incomplete")?;
                    self.at += 1;
                    if matches!(
                        escape,
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't'
                    ) {
                        continue;
                    }
                    if escape != b'u' {
                        return Err("JSON escape is invalid");
                    }
                    let start = self.at;
                    for _ in 0..4 {
                        if !self.bytes.get(self.at).is_some_and(u8::is_ascii_hexdigit) {
                            return Err("JSON Unicode escape is invalid");
                        }
                        self.at += 1;
                    }
                    let code = std::str::from_utf8(&self.bytes[start..self.at])
                        .ok()
                        .and_then(|digits| u16::from_str_radix(digits, 16).ok())
                        .ok_or("JSON Unicode escape is invalid")?;
                    if (0xd800..=0xdbff).contains(&code) {
                        if self.bytes.get(self.at..self.at + 2) != Some(b"\\u") {
                            return Err("JSON Unicode surrogate pair is incomplete");
                        }
                        self.at += 2;
                        let low_start = self.at;
                        for _ in 0..4 {
                            if !self.bytes.get(self.at).is_some_and(u8::is_ascii_hexdigit) {
                                return Err("JSON Unicode surrogate pair is invalid");
                            }
                            self.at += 1;
                        }
                        let low = std::str::from_utf8(&self.bytes[low_start..self.at])
                            .ok()
                            .and_then(|digits| u16::from_str_radix(digits, 16).ok())
                            .ok_or("JSON Unicode surrogate pair is invalid")?;
                        if !(0xdc00..=0xdfff).contains(&low) {
                            return Err("JSON Unicode surrogate pair is invalid");
                        }
                    } else if (0xdc00..=0xdfff).contains(&code) {
                        return Err("JSON Unicode surrogate pair is invalid");
                    }
                }
                _ => {}
            }
        }
        Err("JSON string is unterminated")
    }

    fn number(&mut self) -> Result<(), &'static str> {
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        match self.bytes.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.at += 1;
                while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
            }
            _ => return Err("JSON number is invalid"),
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            if !self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                return Err("JSON number is invalid");
            }
            while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
        }
        if self
            .bytes
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b'e' | b'E'))
        {
            self.at += 1;
            if self
                .bytes
                .get(self.at)
                .is_some_and(|byte| matches!(byte, b'+' | b'-'))
            {
                self.at += 1;
            }
            if !self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                return Err("JSON number is invalid");
            }
            while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                self.at += 1;
            }
        }
        Ok(())
    }

    fn value(&mut self) -> Result<&'static str, &'static str> {
        self.space();
        match self.bytes.get(self.at).copied() {
            Some(b'"') => {
                self.string()?;
                Ok("string")
            }
            Some(b'-' | b'0'..=b'9') => {
                self.number()?;
                Ok("number")
            }
            Some(b'n') if self.bytes.get(self.at..self.at + 4) == Some(b"null") => {
                self.at += 4;
                Ok("null")
            }
            Some(b't') if self.bytes.get(self.at..self.at + 4) == Some(b"true") => {
                self.at += 4;
                Ok("bool")
            }
            Some(b'f') if self.bytes.get(self.at..self.at + 5) == Some(b"false") => {
                self.at += 5;
                Ok("bool")
            }
            Some(open @ (b'[' | b'{')) => {
                if self.depth >= 128 {
                    return Err("JSON nesting exceeds 128 levels");
                }
                self.at += 1;
                self.depth += 1;
                self.space();
                let close = if open == b'[' { b']' } else { b'}' };
                if self.bytes.get(self.at) == Some(&close) {
                    self.at += 1;
                    self.depth -= 1;
                    return Ok(if open == b'[' { "array" } else { "object" });
                }
                let mut object_keys = Vec::new();
                loop {
                    if open == b'{' {
                        let key_start = self.at;
                        self.string()?;
                        let key_end = self.at;
                        let source = std::str::from_utf8(&self.bytes[key_start..key_end])
                            .map_err(|_| "JSON object key is not valid UTF-8")?;
                        let key = json_string_value(source)
                            .map_err(|_| "JSON object key escape is invalid")?;
                        if object_keys.contains(&key) {
                            return Err("JSON object contains a duplicate key");
                        }
                        if object_keys.len() >= 4096 {
                            return Err("JSON object exceeds 4096 keys");
                        }
                        object_keys.push(key);
                        self.space();
                        if self.bytes.get(self.at) != Some(&b':') {
                            return Err("JSON object key is missing ':'");
                        }
                        self.at += 1;
                    }
                    self.value()?;
                    self.space();
                    if self.bytes.get(self.at) == Some(&close) {
                        self.at += 1;
                        self.depth -= 1;
                        return Ok(if open == b'[' { "array" } else { "object" });
                    }
                    if self.bytes.get(self.at) != Some(&b',') {
                        return Err("JSON container is missing ',' or its closing delimiter");
                    }
                    self.at += 1;
                    self.space();
                }
            }
            _ => Err("JSON value is invalid"),
        }
    }
}

fn runtime_json(source: String) -> io::Result<RuntimeJson> {
    if source.len() > HTTP_BODY_LIMIT {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "JSON document exceeds the 16 MiB limit",
        ));
    }
    let mut parser = JsonParser {
        bytes: source.as_bytes(),
        at: 0,
        depth: 0,
    };
    let kind = parser
        .value()
        .map_err(|message| http_error(io::ErrorKind::InvalidData, message))?;
    parser.space();
    if parser.at != parser.bytes.len() {
        return Err(http_error(
            io::ErrorKind::InvalidData,
            "JSON document has trailing data",
        ));
    }
    Ok(RuntimeJson { text: source, kind })
}

fn json_string_value(source: &str) -> io::Result<String> {
    let bytes = source.as_bytes();
    if bytes.len() < 2 || bytes.first() != Some(&b'"') || bytes.last() != Some(&b'"') {
        return Err(http_error(
            io::ErrorKind::InvalidData,
            "JSON value is not a string",
        ));
    }
    let mut result = String::with_capacity(bytes.len().saturating_sub(2));
    let mut at = 1;
    while at + 1 < bytes.len() {
        if bytes[at] != b'\\' {
            let tail = &source[at..bytes.len() - 1];
            let next = tail.find('\\').unwrap_or(tail.len());
            result.push_str(&tail[..next]);
            at += next;
            continue;
        }
        at += 1;
        let escape = bytes[at];
        at += 1;
        match escape {
            b'"' => result.push('"'),
            b'\\' => result.push('\\'),
            b'/' => result.push('/'),
            b'b' => result.push('\u{0008}'),
            b'f' => result.push('\u{000c}'),
            b'n' => result.push('\n'),
            b'r' => result.push('\r'),
            b't' => result.push('\t'),
            b'u' => {
                let decode = |digits: &[u8]| -> io::Result<u16> {
                    let text = std::str::from_utf8(digits).map_err(|_| {
                        http_error(io::ErrorKind::InvalidData, "JSON Unicode escape is invalid")
                    })?;
                    u16::from_str_radix(text, 16).map_err(|_| {
                        http_error(io::ErrorKind::InvalidData, "JSON Unicode escape is invalid")
                    })
                };
                let first = decode(&bytes[at..at + 4])?;
                at += 4;
                let scalar = if (0xd800..=0xdbff).contains(&first) {
                    if bytes.get(at..at + 2) != Some(b"\\u") {
                        return Err(http_error(
                            io::ErrorKind::InvalidData,
                            "JSON Unicode surrogate pair is incomplete",
                        ));
                    }
                    at += 2;
                    let second = decode(&bytes[at..at + 4])?;
                    at += 4;
                    if !(0xdc00..=0xdfff).contains(&second) {
                        return Err(http_error(
                            io::ErrorKind::InvalidData,
                            "JSON Unicode surrogate pair is invalid",
                        ));
                    }
                    0x10000 + (((u32::from(first) - 0xd800) << 10) | (u32::from(second) - 0xdc00))
                } else if (0xdc00..=0xdfff).contains(&first) {
                    return Err(http_error(
                        io::ErrorKind::InvalidData,
                        "JSON Unicode surrogate pair is invalid",
                    ));
                } else {
                    u32::from(first)
                };
                result.push(char::from_u32(scalar).ok_or_else(|| {
                    http_error(io::ErrorKind::InvalidData, "JSON Unicode scalar is invalid")
                })?);
            }
            _ => unreachable!("validated JSON contains only valid escapes"),
        }
    }
    Ok(result)
}

fn json_escape_string(value: &str) -> io::Result<String> {
    let mut length = 2usize;
    for character in value.chars() {
        let additional = match character {
            '"' | '\\' | '\u{0008}' | '\u{000c}' | '\n' | '\r' | '\t' => 2,
            character if character < '\u{0020}' => 6,
            character => character.len_utf8(),
        };
        length = length
            .checked_add(additional)
            .ok_or_else(|| json_conversion_error("JSON document size overflow"))?;
        if length > HTTP_BODY_LIMIT {
            return Err(json_conversion_error(
                "JSON document exceeds the 16 MiB limit",
            ));
        }
    }
    let mut escaped = String::with_capacity(length);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{0008}' => escaped.push_str("\\b"),
            '\u{000c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character < '\u{0020}' => {
                use std::fmt::Write;
                write!(escaped, "\\u{:04x}", character as u32).unwrap();
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    Ok(escaped)
}

fn json_fragment(source: &str, start: usize, end: usize) -> io::Result<RuntimeJson> {
    runtime_json(source[start..end].trim().to_owned())
}

fn json_at(json: &RuntimeJson, index: usize) -> io::Result<Option<RuntimeJson>> {
    if json.kind != "array" {
        return Ok(None);
    }
    let mut parser = JsonParser {
        bytes: json.text.as_bytes(),
        at: 0,
        depth: 0,
    };
    parser.space();
    parser.at += 1;
    parser.space();
    if parser.bytes.get(parser.at) == Some(&b']') {
        return Ok(None);
    }
    let mut current = 0;
    loop {
        parser.space();
        let start = parser.at;
        parser
            .value()
            .map_err(|message| http_error(io::ErrorKind::InvalidData, message))?;
        let end = parser.at;
        if current == index {
            return json_fragment(&json.text, start, end).map(Some);
        }
        current += 1;
        parser.space();
        if parser.bytes.get(parser.at) == Some(&b']') {
            return Ok(None);
        }
        parser.at += 1;
    }
}

fn json_get(json: &RuntimeJson, wanted: &str) -> io::Result<Option<RuntimeJson>> {
    if json.kind != "object" {
        return Ok(None);
    }
    let mut parser = JsonParser {
        bytes: json.text.as_bytes(),
        at: 0,
        depth: 0,
    };
    parser.space();
    parser.at += 1;
    parser.space();
    if parser.bytes.get(parser.at) == Some(&b'}') {
        return Ok(None);
    }
    loop {
        parser.space();
        let key_start = parser.at;
        parser
            .string()
            .map_err(|message| http_error(io::ErrorKind::InvalidData, message))?;
        let key_end = parser.at;
        let key = json_string_value(&json.text[key_start..key_end])?;
        parser.space();
        parser.at += 1;
        parser.space();
        let start = parser.at;
        parser
            .value()
            .map_err(|message| http_error(io::ErrorKind::InvalidData, message))?;
        let end = parser.at;
        if key == wanted {
            return json_fragment(&json.text, start, end).map(Some);
        }
        parser.space();
        if parser.bytes.get(parser.at) == Some(&b'}') {
            return Ok(None);
        }
        parser.at += 1;
    }
}

fn json_conversion_error(message: &str) -> io::Error {
    http_error(io::ErrorKind::InvalidData, message)
}

fn json_array_values(json: &RuntimeJson) -> io::Result<Vec<RuntimeJson>> {
    if json.kind != "array" {
        return Err(json_conversion_error("JSON value is not an array"));
    }
    let mut values = Vec::new();
    let mut parser = JsonParser {
        bytes: json.text.as_bytes(),
        at: 0,
        depth: 0,
    };
    parser.space();
    parser.at += 1;
    parser.space();
    while parser.bytes.get(parser.at) != Some(&b']') {
        let start = parser.at;
        parser
            .value()
            .map_err(|message| http_error(io::ErrorKind::InvalidData, message))?;
        values.push(json_fragment(&json.text, start, parser.at)?);
        parser.space();
        if parser.bytes.get(parser.at) == Some(&b']') {
            break;
        }
        parser.at += 1;
        parser.space();
    }
    Ok(values)
}

fn json_object_entries(json: &RuntimeJson) -> io::Result<Vec<(String, RuntimeJson)>> {
    if json.kind != "object" {
        return Err(json_conversion_error("JSON value is not an object"));
    }
    let mut entries = Vec::new();
    let mut parser = JsonParser {
        bytes: json.text.as_bytes(),
        at: 0,
        depth: 0,
    };
    parser.space();
    parser.at += 1;
    parser.space();
    while parser.bytes.get(parser.at) != Some(&b'}') {
        let key_start = parser.at;
        parser
            .string()
            .map_err(|message| http_error(io::ErrorKind::InvalidData, message))?;
        let key = json_string_value(&json.text[key_start..parser.at])?;
        parser.space();
        parser.at += 1;
        parser.space();
        let start = parser.at;
        parser
            .value()
            .map_err(|message| http_error(io::ErrorKind::InvalidData, message))?;
        entries.push((key, json_fragment(&json.text, start, parser.at)?));
        parser.space();
        if parser.bytes.get(parser.at) == Some(&b'}') {
            break;
        }
        parser.at += 1;
        parser.space();
    }
    Ok(entries)
}

fn json_push(target: &mut String, source: &str) -> io::Result<()> {
    if target
        .len()
        .checked_add(source.len())
        .is_none_or(|length| length > HTTP_BODY_LIMIT)
    {
        return Err(json_conversion_error(
            "JSON document exceeds the 16 MiB limit",
        ));
    }
    target.push_str(source);
    Ok(())
}

fn encode_json_value(program: &Program, value: &Value) -> io::Result<RuntimeJson> {
    let mut text = String::new();
    match value {
        Value::Int(value) => text = value.to_string(),
        Value::UInt(value) => text = value.to_string(),
        Value::Signed(value, _) => text = value.to_string(),
        Value::Unsigned(value, _) => text = value.to_string(),
        Value::Float(value) => {
            if !value.is_finite() {
                return Err(json_conversion_error(
                    "JSON cannot represent NaN or infinity",
                ));
            }
            text = value.to_string();
        }
        Value::Float32(value) => {
            if !value.is_finite() {
                return Err(json_conversion_error(
                    "JSON cannot represent NaN or infinity",
                ));
            }
            text = value.to_string();
        }
        Value::String(value) => text = json_escape_string(&value.text)?,
        Value::Char(value) => text = json_escape_string(&value.to_string())?,
        Value::Bool(value) => text = if *value { "true" } else { "false" }.into(),
        Value::Json(value) => return Ok(value.clone()),
        Value::Unit => text = "null".into(),
        Value::Array(values) | Value::Slice(values) => {
            text.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    text.push(',');
                }
                json_push(&mut text, &encode_json_value(program, value)?.text)?;
            }
            text.push(']');
        }
        Value::List { values, .. } | Value::Set { values, .. } => {
            text.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    text.push(',');
                }
                json_push(&mut text, &encode_json_value(program, value)?.text)?;
            }
            text.push(']');
        }
        Value::Map { entries, .. } => {
            text.push('{');
            for (index, (key, value)) in entries.iter().enumerate() {
                let Value::String(key) = key else {
                    return Err(json_conversion_error(
                        "automatic JSON object keys must be String",
                    ));
                };
                if index != 0 {
                    text.push(',');
                }
                json_push(&mut text, &json_escape_string(&key.text)?)?;
                text.push(':');
                json_push(&mut text, &encode_json_value(program, value)?.text)?;
            }
            text.push('}');
        }
        Value::Struct { type_name, fields } => {
            let declaration = program
                .structs
                .iter()
                .find(|declaration| declaration.name == *type_name)
                .ok_or_else(|| json_conversion_error("unknown struct type during JSON encoding"))?;
            text.push('{');
            for (index, field) in declaration.fields.iter().enumerate() {
                if index != 0 {
                    text.push(',');
                }
                json_push(&mut text, &json_escape_string(&field.name)?)?;
                text.push(':');
                let value = fields.get(&field.name).ok_or_else(|| {
                    json_conversion_error("struct field is missing during JSON encoding")
                })?;
                json_push(&mut text, &encode_json_value(program, value)?.text)?;
            }
            text.push('}');
        }
        Value::Enum {
            type_name,
            variant,
            payload,
        } if type_name == "Option" => {
            if variant == "None" {
                text = "null".into();
            } else {
                return encode_json_value(program, &payload[0]);
            }
        }
        Value::Enum {
            type_name,
            variant,
            payload,
        } if type_name == "Result" => {
            text.push('{');
            json_push(&mut text, &json_escape_string(variant)?)?;
            text.push(':');
            json_push(&mut text, &encode_json_value(program, &payload[0])?.text)?;
            text.push('}');
        }
        Value::Enum {
            variant, payload, ..
        } if payload.is_empty() => text = json_escape_string(variant)?,
        Value::Enum {
            variant, payload, ..
        } => {
            text.push('{');
            json_push(&mut text, &json_escape_string(variant)?)?;
            text.push(':');
            if payload.len() == 1 {
                json_push(&mut text, &encode_json_value(program, &payload[0])?.text)?;
            } else {
                text.push('[');
                for (index, value) in payload.iter().enumerate() {
                    if index != 0 {
                        text.push(',');
                    }
                    json_push(&mut text, &encode_json_value(program, value)?.text)?;
                }
                text.push(']');
            }
            text.push('}');
        }
        _ => return Err(json_conversion_error("value cannot be encoded as JSON")),
    }
    runtime_json(text)
}

fn concrete_json_type(ty: &TypeName, bindings: &HashMap<String, TypeName>) -> TypeName {
    if ty.arguments.is_empty()
        && let Some(bound) = bindings.get(&ty.name)
    {
        return bound.clone();
    }
    let mut concrete = ty.clone();
    concrete.arguments = ty
        .arguments
        .iter()
        .map(|argument| concrete_json_type(argument, bindings))
        .collect();
    concrete
}

fn decode_json_value(
    program: &Program,
    ty: &TypeName,
    bindings: &HashMap<String, TypeName>,
    json: &RuntimeJson,
) -> io::Result<Value> {
    let ty = concrete_json_type(ty, bindings);
    let wrong = |expected: &str| {
        json_conversion_error(&format!("expected {expected}, found JSON {}", json.kind))
    };
    match ty.name.as_str() {
        "Json" if ty.arguments.is_empty() => Ok(Value::Json(json.clone())),
        "String" if ty.arguments.is_empty() => json_string_value(json.text.trim())
            .map(|value| Value::String(RuntimeString::literal(value))),
        "char" if ty.arguments.is_empty() => {
            let text = json_string_value(json.text.trim())?;
            let mut characters = text.chars();
            let value = characters
                .next()
                .filter(|_| characters.next().is_none())
                .ok_or_else(|| {
                    json_conversion_error("JSON char must contain one Unicode scalar")
                })?;
            Ok(Value::Char(value))
        }
        "bool" if ty.arguments.is_empty() => match json.text.trim() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(wrong("bool")),
        },
        "Unit" if ty.arguments.is_empty() => {
            if json.kind == "null" {
                Ok(Value::Unit)
            } else {
                Err(wrong("null"))
            }
        }
        "int" | "i8" | "i16" | "i32" | "i64" | "i128" if ty.arguments.is_empty() => {
            if json.kind != "number" {
                return Err(wrong("signed integer"));
            }
            let value = json.text.trim().parse::<i128>().map_err(|_| {
                json_conversion_error("JSON number is not a representable signed integer")
            })?;
            let width = match ty.name.as_str() {
                "i8" => 8,
                "i16" => 16,
                "i32" => 32,
                "i64" | "int" => 64,
                _ => 128,
            };
            if width < 128 {
                let minimum = -(1_i128 << (width - 1));
                let maximum = (1_i128 << (width - 1)) - 1;
                if !(minimum..=maximum).contains(&value) {
                    return Err(json_conversion_error(
                        "JSON integer is outside the destination type range",
                    ));
                }
            }
            if ty.name == "int" {
                Ok(Value::Int(value as i64))
            } else {
                Ok(Value::Signed(value, width))
            }
        }
        "uint" | "u8" | "u16" | "u32" | "u64" | "u128" if ty.arguments.is_empty() => {
            if json.kind != "number" {
                return Err(wrong("unsigned integer"));
            }
            let value = json.text.trim().parse::<u128>().map_err(|_| {
                json_conversion_error("JSON number is not a representable unsigned integer")
            })?;
            let width = match ty.name.as_str() {
                "u8" => 8,
                "u16" => 16,
                "u32" => 32,
                "u64" | "uint" => 64,
                _ => 128,
            };
            if width < 128 && value >= (1_u128 << width) {
                return Err(json_conversion_error(
                    "JSON integer is outside the destination type range",
                ));
            }
            if ty.name == "uint" {
                Ok(Value::UInt(value as u64))
            } else {
                Ok(Value::Unsigned(value, width))
            }
        }
        "f32" | "f64" if ty.arguments.is_empty() => {
            if json.kind != "number" {
                return Err(wrong("number"));
            }
            let value = json
                .text
                .trim()
                .parse::<f64>()
                .map_err(|_| json_conversion_error("JSON number is not representable"))?;
            if !value.is_finite() {
                return Err(json_conversion_error("JSON number is not finite"));
            }
            if ty.name == "f32" {
                let narrowed = value as f32;
                if !narrowed.is_finite() {
                    return Err(json_conversion_error(
                        "JSON number is outside the f32 range",
                    ));
                }
                Ok(Value::Float32(narrowed))
            } else {
                Ok(Value::Float(value))
            }
        }
        "Option" if ty.arguments.len() == 1 => {
            if json.kind == "null" {
                Ok(Value::Enum {
                    type_name: "Option".into(),
                    variant: "None".into(),
                    payload: vec![],
                })
            } else {
                Ok(Value::Enum {
                    type_name: "Option".into(),
                    variant: "Some".into(),
                    payload: vec![decode_json_value(
                        program,
                        &ty.arguments[0],
                        bindings,
                        json,
                    )?],
                })
            }
        }
        "Result" if ty.arguments.len() == 2 => {
            let entries = json_object_entries(json)?;
            if entries.len() != 1 {
                return Err(json_conversion_error(
                    "JSON Result must contain exactly one `Ok` or `Err` member",
                ));
            }
            let (variant, payload) = &entries[0];
            let index = match variant.as_str() {
                "Ok" => 0,
                "Err" => 1,
                _ => {
                    return Err(json_conversion_error(
                        "JSON Result member must be named `Ok` or `Err`",
                    ));
                }
            };
            Ok(Value::Enum {
                type_name: "Result".into(),
                variant: variant.clone(),
                payload: vec![decode_json_value(
                    program,
                    &ty.arguments[index],
                    bindings,
                    payload,
                )?],
            })
        }
        "List" if ty.arguments.len() == 1 => {
            let values = json_array_values(json)?;
            let values = values
                .iter()
                .map(|value| decode_json_value(program, &ty.arguments[0], bindings, value))
                .collect::<io::Result<Vec<_>>>()?;
            Ok(Value::List {
                capacity: values.len(),
                values,
            })
        }
        "Map" if ty.arguments.len() == 2 && ty.arguments[0].name == "String" => {
            let entries = json_object_entries(json)?;
            let entries = entries
                .iter()
                .map(|(key, value)| {
                    Ok((
                        Value::String(RuntimeString::literal(key.clone())),
                        decode_json_value(program, &ty.arguments[1], bindings, value)?,
                    ))
                })
                .collect::<io::Result<Vec<_>>>()?;
            Ok(Value::Map {
                capacity: entries.len(),
                entries,
            })
        }
        name if name.starts_with("[;") && name.ends_with(']') && ty.arguments.len() == 1 => {
            let expected = name[2..name.len() - 1]
                .parse::<usize>()
                .map_err(|_| json_conversion_error("invalid fixed array type"))?;
            let values = json_array_values(json)?;
            if values.len() != expected {
                return Err(json_conversion_error(
                    "JSON array length does not match the fixed array type",
                ));
            }
            values
                .iter()
                .map(|value| decode_json_value(program, &ty.arguments[0], bindings, value))
                .collect::<io::Result<Vec<_>>>()
                .map(Value::Array)
        }
        name => {
            if let Some(declaration) = program
                .structs
                .iter()
                .find(|declaration| declaration.name == name)
            {
                if declaration.generics.len() != ty.arguments.len() {
                    return Err(json_conversion_error(
                        "nominal JSON type arguments are incomplete",
                    ));
                }
                let nested = declaration
                    .generics
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .zip(ty.arguments.iter().cloned())
                    .collect::<HashMap<_, _>>();
                let entries = json_object_entries(json)?;
                if entries.len() != declaration.fields.len()
                    || entries
                        .iter()
                        .any(|(key, _)| !declaration.fields.iter().any(|field| field.name == *key))
                {
                    return Err(json_conversion_error(&format!(
                        "JSON object does not exactly match struct `{name}`"
                    )));
                }
                let mut fields = HashMap::new();
                for field in &declaration.fields {
                    let value = entries
                        .iter()
                        .find(|(key, _)| key == &field.name)
                        .map(|(_, value)| value)
                        .ok_or_else(|| {
                            json_conversion_error(&format!(
                                "JSON object is missing field `{}`",
                                field.name
                            ))
                        })?;
                    fields.insert(
                        field.name.clone(),
                        decode_json_value(program, &field.ty, &nested, value)?,
                    );
                }
                return Ok(Value::Struct {
                    type_name: name.into(),
                    fields,
                });
            }
            if let Some(declaration) = program
                .enums
                .iter()
                .find(|declaration| declaration.name == name)
            {
                if declaration.generics.len() != ty.arguments.len() {
                    return Err(json_conversion_error(
                        "nominal JSON type arguments are incomplete",
                    ));
                }
                let nested = declaration
                    .generics
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .zip(ty.arguments.iter().cloned())
                    .collect::<HashMap<_, _>>();
                if json.kind == "string" {
                    let variant = json_string_value(json.text.trim())?;
                    let declaration_variant = declaration
                        .variants
                        .iter()
                        .find(|candidate| candidate.name == variant && candidate.payload.is_empty())
                        .ok_or_else(|| {
                            json_conversion_error(&format!(
                                "unknown unit variant `{variant}` for enum `{name}`"
                            ))
                        })?;
                    return Ok(Value::Enum {
                        type_name: name.into(),
                        variant: declaration_variant.name.clone(),
                        payload: vec![],
                    });
                }
                let entries = json_object_entries(json)?;
                if entries.len() != 1 {
                    return Err(json_conversion_error(&format!(
                        "JSON enum `{name}` must contain exactly one variant member"
                    )));
                }
                let (variant, value) = &entries[0];
                let declaration_variant = declaration
                    .variants
                    .iter()
                    .find(|candidate| candidate.name == *variant && !candidate.payload.is_empty())
                    .ok_or_else(|| {
                        json_conversion_error(&format!(
                            "unknown payload variant `{variant}` for enum `{name}`"
                        ))
                    })?;
                let payload_json = if declaration_variant.payload.len() == 1 {
                    vec![value.clone()]
                } else {
                    let values = json_array_values(value)?;
                    if values.len() != declaration_variant.payload.len() {
                        return Err(json_conversion_error(&format!(
                            "JSON payload for `{name}.{variant}` has the wrong length"
                        )));
                    }
                    values
                };
                let payload = declaration_variant
                    .payload
                    .iter()
                    .zip(&payload_json)
                    .map(|(ty, value)| decode_json_value(program, ty, &nested, value))
                    .collect::<io::Result<Vec<_>>>()?;
                return Ok(Value::Enum {
                    type_name: name.into(),
                    variant: variant.clone(),
                    payload,
                });
            }
            Err(json_conversion_error("type cannot be decoded from JSON"))
        }
    }
}

struct RuntimeTcpStreamState {
    socket: Option<StdTcpStream>,
    read_shutdown: bool,
    write_shutdown: bool,
}

impl RuntimeTcpStream {
    fn new(socket: StdTcpStream) -> Self {
        Self(Arc::new(StdMutex::new(RuntimeTcpStreamState {
            socket: Some(socket),
            read_shutdown: false,
            write_shutdown: false,
        })))
    }
}

#[derive(Clone)]
struct RuntimeTcpListener(Arc<StdMutex<Option<StdTcpListener>>>);

#[derive(Clone)]
struct RuntimeUdpSocket(Arc<StdMutex<Option<StdUdpSocket>>>);

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeUdpDatagram {
    source: RuntimeSocketAddress,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for RuntimeTcpStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("TcpStream")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeTcpStream {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for RuntimeTlsStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("TlsStream")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeTlsStream {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for RuntimeTcpListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("TcpListener")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeTcpListener {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for RuntimeUdpSocket {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("UdpSocket")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeUdpSocket {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl RuntimeFuture {
    fn new(function: Function, arguments: Vec<Value>) -> Self {
        Self(Arc::new(StdMutex::new(Some(FutureWork::Function(
            Box::new(function),
            arguments,
        )))))
    }

    fn yielding() -> Self {
        Self(Arc::new(StdMutex::new(Some(FutureWork::Yield))))
    }

    fn operation(work: FutureWork) -> Self {
        Self(Arc::new(StdMutex::new(Some(work))))
    }
}

impl std::fmt::Debug for RuntimeFuture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Future")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeFuture {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeTask(Arc<StdMutex<RuntimeTaskWork>>);

enum RuntimeTaskWork {
    Future(RuntimeFuture),
    Running,
    Ready(Value),
    Consumed,
}

impl RuntimeTask {
    fn new(future: RuntimeFuture) -> Self {
        Self(Arc::new(StdMutex::new(RuntimeTaskWork::Future(future))))
    }
}

impl std::fmt::Debug for RuntimeTask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Task")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeTask {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeMutex(Arc<RuntimeMutexState>);

struct RuntimeMutexState {
    locked: AtomicBool,
    value: StdMutex<Box<dyn Any + Send>>,
}

impl RuntimeMutex {
    fn new(value: Value) -> Self {
        Self(Arc::new(RuntimeMutexState {
            locked: AtomicBool::new(false),
            value: StdMutex::new(Box::new(value)),
        }))
    }

    fn lock(&self) -> RuntimeMutexGuard {
        while self
            .0
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            thread::yield_now();
        }
        RuntimeMutexGuard(Arc::new(RuntimeMutexGuardState {
            mutex: Arc::clone(&self.0),
        }))
    }
}

impl std::fmt::Debug for RuntimeMutex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Mutex")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeMutex {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeMutexGuard(Arc<RuntimeMutexGuardState>);

struct RuntimeMutexGuardState {
    mutex: Arc<RuntimeMutexState>,
}

impl RuntimeMutexGuard {
    fn read(&self) -> Option<Value> {
        self.0
            .mutex
            .value
            .lock()
            .ok()?
            .downcast_ref::<Value>()
            .cloned()
    }

    fn write(&self, value: Value) -> Option<()> {
        *self.0.mutex.value.lock().ok()? = Box::new(value);
        Some(())
    }
}

impl Drop for RuntimeMutexGuardState {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for RuntimeMutexGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("MutexGuard")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeMutexGuard {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeAtomicInt(Arc<AtomicI64>);

impl RuntimeAtomicInt {
    fn add(&self, delta: i64, span: Span) -> Result<(i64, i64), Diagnostic> {
        let mut current = self.0.load(Ordering::SeqCst);
        loop {
            let next = current.checked_add(delta).ok_or_else(|| {
                Diagnostic::new(DiagnosticKind::Runtime, "AtomicInt overflow", span)
            })?;
            match self
                .0
                .compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(previous) => return Ok((previous, next)),
                Err(observed) => current = observed,
            }
        }
    }
}

impl std::fmt::Debug for RuntimeAtomicInt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("AtomicInt")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeAtomicInt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeString {
    text: String,
    capacity: usize,
}

impl RuntimeString {
    fn literal(text: String) -> Self {
        let capacity = text.len();
        Self { text, capacity }
    }
    fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::new(),
            capacity,
        }
    }
    fn len(&self) -> usize {
        self.text.len()
    }
    fn capacity(&self) -> usize {
        self.capacity.max(self.text.len())
    }
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    fn push(&mut self, value: char) {
        self.text.push(value);
        self.capacity = self.capacity.max(self.text.capacity());
    }
    fn push_str(&mut self, value: &RuntimeString) {
        self.text.push_str(&value.text);
        self.capacity = self.capacity.max(self.text.capacity());
    }
    fn clear(&mut self) {
        self.text.clear();
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeCString(Arc<Vec<u8>>);

impl RuntimeCString {
    fn new(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.contains(&0) {
            return Err("CString source contains an interior NUL byte");
        }
        let mut terminated = Vec::with_capacity(bytes.len() + 1);
        terminated.extend_from_slice(bytes);
        terminated.push(0);
        Ok(Self(Arc::new(terminated)))
    }

    fn len(&self) -> usize {
        self.0.len().saturating_sub(1)
    }

    fn text(&self) -> String {
        String::from_utf8(self.0[..self.len()].to_vec())
            .expect("CString is created only from valid DISP UTF-8")
    }
}

#[derive(Debug)]
struct RuntimeMemoryState {
    bytes: Vec<u8>,
    alignment: usize,
}

#[derive(Debug, Clone)]
struct RuntimeMemory(Arc<StdMutex<RuntimeMemoryState>>);

impl PartialEq for RuntimeMemory {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl RuntimeMemory {
    fn new(size: usize, alignment: usize) -> Result<Self, &'static str> {
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err("Memory alignment must be a non-zero power of two");
        }
        if alignment > 1 << 20 {
            return Err("Memory alignment exceeds the supported maximum");
        }
        if size
            .checked_add(3 * std::mem::size_of::<usize>())
            .and_then(|value| value.checked_add(alignment - 1))
            .is_none()
        {
            return Err("Memory size overflow");
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| "Memory allocation failed")?;
        bytes.resize(size, 0);
        Ok(Self(Arc::new(StdMutex::new(RuntimeMemoryState {
            bytes,
            alignment,
        }))))
    }

    fn len(&self) -> Option<usize> {
        Some(self.0.lock().ok()?.bytes.len())
    }

    fn alignment(&self) -> Option<usize> {
        Some(self.0.lock().ok()?.alignment)
    }

    fn read(&self, index: usize) -> Option<u8> {
        self.0.lock().ok()?.bytes.get(index).copied()
    }

    fn write(&self, index: usize, value: u8) -> Option<()> {
        *self.0.lock().ok()?.bytes.get_mut(index)? = value;
        Some(())
    }

    fn fill(&self, value: u8) -> Option<()> {
        self.0.lock().ok()?.bytes.fill(value);
        Some(())
    }

    fn copy_from(
        &self,
        destination: usize,
        source: &Self,
        source_offset: usize,
        count: usize,
    ) -> Option<()> {
        let destination_end = destination.checked_add(count)?;
        let source_end = source_offset.checked_add(count)?;
        if Arc::ptr_eq(&self.0, &source.0) {
            let mut state = self.0.lock().ok()?;
            if destination_end > state.bytes.len() || source_end > state.bytes.len() {
                return None;
            }
            state
                .bytes
                .copy_within(source_offset..source_end, destination);
            return Some(());
        }
        let copied = {
            let state = source.0.lock().ok()?;
            state.bytes.get(source_offset..source_end)?.to_vec()
        };
        let mut state = self.0.lock().ok()?;
        state
            .bytes
            .get_mut(destination..destination_end)?
            .copy_from_slice(&copied);
        Some(())
    }
}

fn grow_list_capacity(capacity: &mut usize, needed: usize) -> Result<(), &'static str> {
    if needed <= *capacity {
        return Ok(());
    }
    let mut next = (*capacity).max(4);
    while next < needed {
        next = next.checked_mul(2).ok_or("List capacity overflow")?;
    }
    *capacity = next;
    Ok(())
}

fn option_value(value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::Enum {
            type_name: "Option".into(),
            variant: "Some".into(),
            payload: vec![value],
        },
        None => Value::Enum {
            type_name: "Option".into(),
            variant: "None".into(),
            payload: vec![],
        },
    }
}

fn runtime_result(value: Result<Value, std::io::Error>) -> Value {
    match value {
        Ok(value) => Value::Enum {
            type_name: "Result".into(),
            variant: "Ok".into(),
            payload: vec![value],
        },
        Err(error) => Value::Enum {
            type_name: "Result".into(),
            variant: "Err".into(),
            payload: vec![Value::String(RuntimeString::literal(error.to_string()))],
        },
    }
}

fn runtime_bytes(bytes: Vec<u8>) -> Value {
    Value::List {
        capacity: bytes.len(),
        values: bytes
            .into_iter()
            .map(|byte| Value::Unsigned(byte as u128, 8))
            .collect(),
    }
}

fn runtime_http_body(value: Value) -> Option<Vec<u8>> {
    match value {
        Value::String(text) => Some(text.text.into_bytes()),
        Value::Json(json) => Some(json.text.into_bytes()),
        Value::List { values, .. } | Value::Slice(values) => values
            .into_iter()
            .map(|value| match value {
                Value::Unsigned(byte, 8) => Some(byte as u8),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

const HTTP_HEADER_LIMIT: usize = 64 * 1024;
const HTTP_BODY_LIMIT: usize = 16 * 1024 * 1024;
const HTTP_CHUNKED_WIRE_LIMIT: usize = HTTP_BODY_LIMIT + 1024 * 1024;
const HTTP_REDIRECT_LIMIT: usize = 10;

enum InterpreterHttpStream {
    Plain(StdTcpStream),
    Tls(Box<NativeTlsStream<StdTcpStream>>),
}

type HttpHeaders = Vec<(String, String)>;
type ParsedHttpHeaders = (u16, HttpHeaders, usize, bool);
type ParsedHttpResponse = (u16, HttpHeaders, Vec<u8>, bool);
type InterpreterHttpPool = HashMap<String, Vec<InterpreterHttpStream>>;

impl Read for InterpreterHttpStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.read(buffer),
            Self::Tls(stream) => stream.read(buffer),
        }
    }
}

impl Write for InterpreterHttpStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(stream) => stream.write(buffer),
            Self::Tls(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(stream) => stream.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

fn http_error(kind: io::ErrorKind, message: impl Into<String>) -> io::Error {
    io::Error::new(kind, message.into())
}

fn http_remaining(deadline: StdInstant) -> io::Result<StdDuration> {
    deadline
        .checked_duration_since(StdInstant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| http_error(io::ErrorKind::TimedOut, "HTTP request timed out"))
}

fn parse_http_url(source: &str) -> io::Result<Url> {
    if source.is_empty()
        || source.len() > HTTP_URL_LIMIT
        || source
            .bytes()
            .any(|byte| byte == 0 || byte <= 0x20 || byte == 0x7f)
    {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP URL must be non-empty, at most 8192 bytes, and contain no control characters or spaces",
        ));
    }
    let url = Url::parse(source).map_err(|error| {
        http_error(
            io::ErrorKind::InvalidInput,
            format!("invalid HTTP URL: {error}"),
        )
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP URL scheme must be http or https",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "credentials are not allowed in HTTP URLs",
        ));
    }
    if url.fragment().is_some() || url.host_str().is_none() {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP URL must contain a host and must not contain a fragment",
        ));
    }
    Ok(url)
}

fn connect_http_stream(url: &Url, deadline: StdInstant) -> io::Result<InterpreterHttpStream> {
    let host = url.host_str().ok_or_else(|| {
        http_error(
            io::ErrorKind::InvalidInput,
            "HTTP URL does not contain a host",
        )
    })?;
    let port = url.port_or_known_default().ok_or_else(|| {
        http_error(
            io::ErrorKind::InvalidInput,
            "HTTP URL does not contain a valid port",
        )
    })?;
    let addresses = (host, port).to_socket_addrs()?;
    let mut last_error = None;
    let mut socket = None;
    for address in addresses {
        match StdTcpStream::connect_timeout(&address, http_remaining(deadline)?) {
            Ok(connected) => {
                socket = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let socket = socket.ok_or_else(|| {
        last_error.unwrap_or_else(|| {
            http_error(
                io::ErrorKind::AddrNotAvailable,
                "HTTP host resolved to no reachable address",
            )
        })
    })?;
    socket.set_read_timeout(Some(http_remaining(deadline)?))?;
    socket.set_write_timeout(Some(http_remaining(deadline)?))?;
    if url.scheme() == "http" {
        return Ok(InterpreterHttpStream::Plain(socket));
    }
    let mut builder = TlsConnector::builder();
    builder.min_protocol_version(Some(Protocol::Tlsv12));
    let connector = builder.build().map_err(tls_error)?;
    connector
        .connect(host, socket)
        .map(|stream| InterpreterHttpStream::Tls(Box::new(stream)))
        .map_err(tls_error)
}

fn http_request_target(url: &Url) -> &str {
    let target = &url[Position::BeforePath..Position::AfterQuery];
    if target.is_empty() { "/" } else { target }
}

fn http_host_header(url: &Url) -> io::Result<String> {
    let host = match url.host().ok_or_else(|| {
        http_error(
            io::ErrorKind::InvalidInput,
            "HTTP URL does not contain a host",
        )
    })? {
        Host::Ipv6(address) => format!("[{address}]"),
        other => other.to_string(),
    };
    let default = match url.scheme() {
        "http" => 80,
        "https" => 443,
        _ => unreachable!(),
    };
    Ok(match url.port() {
        Some(port) if port != default => format!("{host}:{port}"),
        _ => host,
    })
}

fn http_origin(url: &Url) -> io::Result<String> {
    Ok(format!("{}://{}", url.scheme(), http_host_header(url)?))
}

fn http_stream_timeout(stream: &InterpreterHttpStream, timeout: StdDuration) -> io::Result<()> {
    match stream {
        InterpreterHttpStream::Plain(socket) => {
            socket.set_read_timeout(Some(timeout))?;
            socket.set_write_timeout(Some(timeout))
        }
        InterpreterHttpStream::Tls(socket) => {
            socket.get_ref().set_read_timeout(Some(timeout))?;
            socket.get_ref().set_write_timeout(Some(timeout))
        }
    }
}

fn http_pool_return(pool: &mut InterpreterHttpPool, origin: String, stream: InterpreterHttpStream) {
    if pool.len() < 32 || pool.contains_key(&origin) {
        let streams = pool.entry(origin).or_default();
        if streams.len() < 2 {
            streams.push(stream);
        }
    }
}

fn find_crlf(bytes: &[u8], start: usize) -> Option<usize> {
    bytes
        .get(start..)?
        .windows(2)
        .position(|window| window == b"\r\n")
        .map(|offset| start + offset)
}

fn decode_chunked_body(bytes: &[u8]) -> io::Result<Option<(Vec<u8>, usize)>> {
    let mut position = 0;
    let mut decoded = Vec::new();
    loop {
        let Some(line_end) = find_crlf(bytes, position) else {
            if bytes.len().saturating_sub(position) > 8192 {
                return Err(http_error(
                    io::ErrorKind::InvalidData,
                    "HTTP chunk line exceeds 8192 bytes",
                ));
            }
            return Ok(None);
        };
        if line_end - position > 8192 {
            return Err(http_error(
                io::ErrorKind::InvalidData,
                "HTTP chunk line exceeds 8192 bytes",
            ));
        }
        let line = std::str::from_utf8(&bytes[position..line_end])
            .map_err(|_| http_error(io::ErrorKind::InvalidData, "HTTP chunk size is not ASCII"))?;
        let digits = line.split(';').next().unwrap_or_default();
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(http_error(
                io::ErrorKind::InvalidData,
                "invalid HTTP chunk size",
            ));
        }
        let size = usize::from_str_radix(digits, 16)
            .map_err(|_| http_error(io::ErrorKind::InvalidData, "HTTP chunk size overflow"))?;
        position = line_end + 2;
        if size == 0 {
            loop {
                let Some(trailer_end) = find_crlf(bytes, position) else {
                    return Ok(None);
                };
                if trailer_end == position {
                    return Ok(Some((decoded, trailer_end + 2)));
                }
                if trailer_end - position > 8192 {
                    return Err(http_error(
                        io::ErrorKind::InvalidData,
                        "HTTP trailer line exceeds 8192 bytes",
                    ));
                }
                position = trailer_end + 2;
            }
        }
        let end = position
            .checked_add(size)
            .ok_or_else(|| http_error(io::ErrorKind::InvalidData, "HTTP chunk size overflow"))?;
        let framed_end = end
            .checked_add(2)
            .ok_or_else(|| http_error(io::ErrorKind::InvalidData, "HTTP chunk size overflow"))?;
        if framed_end > bytes.len() {
            return Ok(None);
        }
        if &bytes[end..framed_end] != b"\r\n" {
            return Err(http_error(
                io::ErrorKind::InvalidData,
                "HTTP chunk is missing its terminator",
            ));
        }
        if decoded.len().saturating_add(size) > HTTP_BODY_LIMIT {
            return Err(http_error(
                io::ErrorKind::InvalidData,
                "HTTP response body exceeds the 16 MiB limit",
            ));
        }
        decoded.extend_from_slice(&bytes[position..end]);
        position = framed_end;
    }
}

fn parse_http_headers(bytes: &[u8]) -> io::Result<ParsedHttpHeaders> {
    let mut storage = [httparse::EMPTY_HEADER; 100];
    let mut response = httparse::Response::new(&mut storage);
    let consumed = match response.parse(bytes).map_err(|error| {
        http_error(
            io::ErrorKind::InvalidData,
            format!("invalid HTTP response: {error}"),
        )
    })? {
        httparse::Status::Complete(consumed) => consumed,
        httparse::Status::Partial => {
            return Err(http_error(
                io::ErrorKind::UnexpectedEof,
                "incomplete HTTP response headers",
            ));
        }
    };
    let status = response.code.ok_or_else(|| {
        http_error(
            io::ErrorKind::InvalidData,
            "HTTP response has no status code",
        )
    })?;
    let mut headers = Vec::with_capacity(response.headers.len());
    for header in response.headers {
        let value = std::str::from_utf8(header.value).map_err(|_| {
            http_error(
                io::ErrorKind::InvalidData,
                "HTTP response header is not valid UTF-8",
            )
        })?;
        headers.push((header.name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    Ok((status, headers, consumed, response.version == Some(1)))
}

fn read_http_response(
    stream: &mut InterpreterHttpStream,
    deadline: StdInstant,
    head_request: bool,
) -> io::Result<ParsedHttpResponse> {
    let mut wire = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    let (status, headers, body_start, http11) = loop {
        if let Some(end) = wire.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = end + 4;
            if header_end > HTTP_HEADER_LIMIT {
                return Err(http_error(
                    io::ErrorKind::InvalidData,
                    "HTTP response headers exceed the 64 KiB limit",
                ));
            }
            let (status, headers, consumed, http11) = parse_http_headers(&wire[..header_end])?;
            if consumed != header_end {
                return Err(http_error(
                    io::ErrorKind::InvalidData,
                    "ambiguous HTTP response headers",
                ));
            }
            if (100..200).contains(&status) && status != 101 {
                wire.drain(..header_end);
                continue;
            }
            break (status, headers, header_end, http11);
        }
        if wire.len() >= HTTP_HEADER_LIMIT {
            return Err(http_error(
                io::ErrorKind::InvalidData,
                "HTTP response headers exceed the 64 KiB limit",
            ));
        }
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(http_error(
                io::ErrorKind::UnexpectedEof,
                "connection ended before HTTP response headers",
            ));
        }
        wire.extend_from_slice(&chunk[..count]);
    };
    let content_lengths = headers
        .iter()
        .filter(|(name, _)| name == "content-length")
        .map(|(_, value)| value.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| http_error(io::ErrorKind::InvalidData, "invalid HTTP Content-Length"))?;
    if content_lengths.windows(2).any(|pair| pair[0] != pair[1]) {
        return Err(http_error(
            io::ErrorKind::InvalidData,
            "conflicting HTTP Content-Length headers",
        ));
    }
    let content_length = content_lengths.first().copied();
    let transfer_codings = headers
        .iter()
        .filter(|(name, _)| name == "transfer-encoding")
        .flat_map(|(_, value)| value.split(','))
        .map(str::trim)
        .collect::<Vec<_>>();
    if !transfer_codings.is_empty() && content_length.is_some() {
        return Err(http_error(
            io::ErrorKind::InvalidData,
            "ambiguous HTTP body framing",
        ));
    }
    let chunked =
        transfer_codings.len() == 1 && transfer_codings[0].eq_ignore_ascii_case("chunked");
    if !transfer_codings.is_empty() && !chunked {
        return Err(http_error(
            io::ErrorKind::InvalidData,
            "unsupported HTTP Transfer-Encoding",
        ));
    }
    let reusable = http11
        && !headers.iter().any(|(name, value)| {
            name == "connection"
                && value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("close"))
        });
    let no_body = head_request || matches!(status, 204 | 304) || (100..200).contains(&status);
    if no_body {
        return Ok((status, headers, Vec::new(), reusable));
    }
    if let Some(length) = content_length {
        if length > HTTP_BODY_LIMIT {
            return Err(http_error(
                io::ErrorKind::InvalidData,
                "HTTP response body exceeds the 16 MiB limit",
            ));
        }
        while wire.len().saturating_sub(body_start) < length {
            let remaining = http_remaining(deadline)?;
            match stream {
                InterpreterHttpStream::Plain(socket) => socket.set_read_timeout(Some(remaining))?,
                InterpreterHttpStream::Tls(socket) => {
                    socket.get_ref().set_read_timeout(Some(remaining))?
                }
            }
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                return Err(http_error(
                    io::ErrorKind::UnexpectedEof,
                    "HTTP response body ended early",
                ));
            }
            wire.extend_from_slice(&chunk[..count]);
        }
        return Ok((
            status,
            headers,
            wire[body_start..body_start + length].to_vec(),
            reusable && wire.len() == body_start + length,
        ));
    }
    if chunked {
        loop {
            let encoded = &wire[body_start..];
            if encoded.len() > HTTP_CHUNKED_WIRE_LIMIT {
                return Err(http_error(
                    io::ErrorKind::InvalidData,
                    "HTTP chunk framing exceeds its safety limit",
                ));
            }
            if let Some((body, consumed)) = decode_chunked_body(encoded)? {
                return Ok((status, headers, body, reusable && encoded.len() == consumed));
            }
            let remaining = http_remaining(deadline)?;
            match stream {
                InterpreterHttpStream::Plain(socket) => socket.set_read_timeout(Some(remaining))?,
                InterpreterHttpStream::Tls(socket) => {
                    socket.get_ref().set_read_timeout(Some(remaining))?
                }
            }
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                return Err(http_error(
                    io::ErrorKind::UnexpectedEof,
                    "chunked HTTP response ended early",
                ));
            }
            wire.extend_from_slice(&chunk[..count]);
        }
    }
    loop {
        if wire.len().saturating_sub(body_start) > HTTP_BODY_LIMIT {
            return Err(http_error(
                io::ErrorKind::InvalidData,
                "HTTP response body exceeds the 16 MiB limit",
            ));
        }
        let remaining = http_remaining(deadline)?;
        match stream {
            InterpreterHttpStream::Plain(socket) => socket.set_read_timeout(Some(remaining))?,
            InterpreterHttpStream::Tls(socket) => {
                socket.get_ref().set_read_timeout(Some(remaining))?
            }
        }
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Ok((status, headers, wire[body_start..].to_vec(), false));
        }
        wire.extend_from_slice(&chunk[..count]);
    }
}

fn http_method(method: &str) -> io::Result<String> {
    let upper = method.to_ascii_uppercase();
    if method.is_empty()
        || method.len() > 32
        || !method.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
        || matches!(upper.as_str(), "CONNECT" | "TRACE")
    {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP method is invalid or forbidden by the safe client",
        ));
    }
    Ok(upper)
}

fn http_forbidden_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host"
            | "content-length"
            | "transfer-encoding"
            | "connection"
            | "proxy-connection"
            | "proxy-authorization"
            | "trailer"
            | "te"
            | "upgrade"
    )
}

fn http_header(name: &str, value: &str) -> io::Result<()> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
        || http_forbidden_header(name)
    {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP header name is invalid or controlled by the safe client",
        ));
    }
    if value
        .bytes()
        .any(|byte| byte > 0x7e || (byte < 0x20 && byte != b'\t'))
    {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP header value must contain only safe ASCII text",
        ));
    }
    Ok(())
}

fn http_request_size(request: &RuntimeHttpRequest) -> io::Result<()> {
    if request.headers.len() > 100 {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP request contains more than 100 headers",
        ));
    }
    let header_bytes = request
        .headers
        .iter()
        .try_fold(0usize, |total, (name, value)| {
            total
                .checked_add(name.len())
                .and_then(|total| total.checked_add(value.len() + 4))
                .ok_or_else(|| {
                    http_error(
                        io::ErrorKind::InvalidInput,
                        "HTTP request header size overflow",
                    )
                })
        })?;
    if header_bytes > HTTP_HEADER_LIMIT {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP request headers exceed the 64 KiB limit",
        ));
    }
    if request.body.len() > HTTP_BODY_LIMIT {
        return Err(http_error(
            io::ErrorKind::InvalidInput,
            "HTTP request body exceeds the 16 MiB limit",
        ));
    }
    Ok(())
}

fn interpreter_http_request(
    request: RuntimeHttpRequest,
    timeout: StdDuration,
    pool: &mut InterpreterHttpPool,
) -> io::Result<Value> {
    if timeout.is_zero() {
        return Err(http_error(
            io::ErrorKind::TimedOut,
            "HTTP request timed out",
        ));
    }
    let deadline = StdInstant::now()
        .checked_add(timeout)
        .ok_or_else(|| http_error(io::ErrorKind::InvalidInput, "HTTP timeout is too large"))?;
    http_request_size(&request)?;
    let method = http_method(&request.method)?;
    let mut url = parse_http_url(&request.url)?;
    for redirects in 0..=HTTP_REDIRECT_LIMIT {
        let origin = http_origin(&url)?;
        let mut stream = if let Some(stream) = pool.get_mut(&origin).and_then(Vec::pop) {
            http_stream_timeout(&stream, http_remaining(deadline)?)?;
            stream
        } else {
            connect_http_stream(&url, deadline)?
        };
        let mut wire = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: DISP/0.1\r\nAccept: */*\r\nConnection: keep-alive\r\n",
            method,
            http_request_target(&url),
            http_host_header(&url)?
        );
        for (name, value) in &request.headers {
            http_header(name, value)?;
            wire.push_str(name);
            wire.push_str(": ");
            wire.push_str(value);
            wire.push_str("\r\n");
        }
        if !request.body.is_empty() || matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
            wire.push_str(&format!("Content-Length: {}\r\n", request.body.len()));
        }
        wire.push_str("\r\n");
        stream.write_all(wire.as_bytes())?;
        stream.write_all(&request.body)?;
        stream.flush()?;
        let (status, headers, body, reusable) =
            read_http_response(&mut stream, deadline, method == "HEAD")?;
        let location = headers
            .iter()
            .find(|(name, _)| name == "location")
            .map(|(_, value)| value.as_str());
        if let (true, Some(location)) = (
            matches!(method.as_str(), "GET" | "HEAD")
                && request.headers.is_empty()
                && request.body.is_empty()
                && matches!(status, 301 | 302 | 303 | 307 | 308),
            location,
        ) {
            if reusable {
                http_pool_return(pool, origin, stream);
            }
            if redirects == HTTP_REDIRECT_LIMIT {
                return Err(http_error(
                    io::ErrorKind::InvalidData,
                    "HTTP redirect limit exceeded",
                ));
            }
            let next = url.join(location).map_err(|error| {
                http_error(
                    io::ErrorKind::InvalidData,
                    format!("invalid HTTP redirect URL: {error}"),
                )
            })?;
            let next = parse_http_url(next.as_str())?;
            if url.scheme() == "https" && next.scheme() != "https" {
                return Err(http_error(
                    io::ErrorKind::PermissionDenied,
                    "HTTPS to HTTP redirect is rejected",
                ));
            }
            url = next;
            continue;
        }
        if reusable {
            http_pool_return(pool, origin, stream);
        }
        return Ok(Value::HttpResponse(RuntimeHttpResponse(Arc::new(
            RuntimeHttpResponseData {
                status,
                url: url.to_string(),
                headers,
                body,
            },
        ))));
    }
    unreachable!()
}

fn tls_error(error: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(format!("TLS: {error}"))
}

fn runtime_path(value: Value) -> Option<PathBuf> {
    match value {
        Value::Path(path) => Some(path),
        Value::String(text) => Some(PathBuf::from(text.text)),
        Value::Reference(_, _) => None,
        _ => None,
    }
}

fn resolve_ip_addresses(host: &str) -> std::io::Result<Vec<IpAddr>> {
    let mut addresses = (host, 0)
        .to_socket_addrs()?
        .map(|address| address.ip())
        .collect::<Vec<_>>();
    addresses.sort();
    addresses.dedup();
    if addresses.is_empty() {
        Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "DNS resolution returned no addresses",
        ))
    } else {
        Ok(addresses)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i64),
    UInt(u64),
    Signed(i128, u16),
    Unsigned(u128, u16),
    Float(f64),
    Float32(f32),
    Reference(Place, bool),
    String(RuntimeString),
    CString(RuntimeCString),
    CStr(RuntimeCString),
    Memory(RuntimeMemory),
    Path(PathBuf),
    Url(RuntimeUrl),
    Json(RuntimeJson),
    IpAddress(RuntimeIpAddress),
    SocketAddress(RuntimeSocketAddress),
    TcpStream(RuntimeTcpStream),
    TlsStream(RuntimeTlsStream),
    HttpRequest(RuntimeHttpRequest),
    HttpResponse(RuntimeHttpResponse),
    TcpListener(RuntimeTcpListener),
    UdpSocket(RuntimeUdpSocket),
    UdpDatagram(RuntimeUdpDatagram),
    Instant(StdInstant),
    Duration(StdDuration),
    ProcessCommand(Box<RuntimeProcessCommand>),
    ChildProcess(RuntimeChildProcess),
    ProcessOutput(RuntimeProcessOutput),
    Database(RuntimeDatabase),
    Thread(RuntimeThread),
    Future(RuntimeFuture),
    Task(RuntimeTask),
    Mutex(RuntimeMutex),
    MutexGuard(RuntimeMutexGuard),
    AtomicInt(RuntimeAtomicInt),
    Array(Vec<Value>),
    Slice(Vec<Value>),
    List {
        values: Vec<Value>,
        capacity: usize,
    },
    Map {
        entries: Vec<(Value, Value)>,
        capacity: usize,
    },
    Set {
        values: Vec<Value>,
        capacity: usize,
    },
    Char(char),
    Bool(bool),
    Function(String),
    Closure(Box<RuntimeClosure>),
    CaptureReference(Place, bool),
    Constructor {
        type_name: String,
        variant: String,
    },
    Struct {
        type_name: String,
        fields: HashMap<String, Value>,
    },
    Enum {
        type_name: String,
        variant: String,
        payload: Vec<Value>,
    },
    Unit,
    Uninitialized,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeProcessOutput {
    status: i64,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeProcessCommand {
    program: PathBuf,
    arguments: Vec<String>,
    directory: Option<PathBuf>,
    environment: Vec<(String, String)>,
    clear_environment: bool,
    input: Vec<u8>,
    timeout: Option<StdDuration>,
}

const SQLITE_OK: i32 = 0;
const SQLITE_ROW: i32 = 100;
const SQLITE_DONE: i32 = 101;
const SQLITE_INTEGER: i32 = 1;
const SQLITE_FLOAT: i32 = 2;
const SQLITE_TEXT: i32 = 3;
const SQLITE_BLOB: i32 = 4;
const SQLITE_NULL: i32 = 5;
const DATABASE_SQL_LIMIT: usize = 1024 * 1024;
const DATABASE_ROW_LIMIT: usize = 100_000;

#[derive(Clone, PartialEq, Eq)]
struct NativeDataField {
    name: String,
    storage: &'static str,
    optional: bool,
    primary: bool,
}

struct NativeDataTable {
    fields: Vec<NativeDataField>,
    primary: usize,
    rows: Vec<Value>,
}

#[derive(Default)]
struct NativeDataStore {
    tables: HashMap<String, NativeDataTable>,
}

struct RuntimeDatabaseState {
    handle: *mut Sqlite3,
    closed: bool,
    native: Option<NativeDataStore>,
}

// SQLite is opened in FULLMUTEX mode, and every access is additionally serialized
// through RuntimeDatabase's mutex before the raw connection is touched.
unsafe impl Send for RuntimeDatabaseState {}

#[derive(Clone)]
struct RuntimeDatabase(Arc<StdMutex<RuntimeDatabaseState>>);

impl std::fmt::Debug for RuntimeDatabase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("Database").finish()
    }
}

impl PartialEq for RuntimeDatabase {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

struct RuntimeStatement(*mut SqliteStatement);

impl Drop for RuntimeStatement {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper uniquely owns the prepared statement.
            unsafe { sqlite3_finalize(self.0) };
            self.0 = std::ptr::null_mut();
        }
    }
}

fn database_error(handle: *mut Sqlite3, fallback: &str) -> io::Error {
    if handle.is_null() {
        return io::Error::other(fallback.to_owned());
    }
    // SAFETY: SQLite keeps the error string valid until the next API call on this handle.
    let message = unsafe {
        let pointer = sqlite3_errmsg(handle);
        (!pointer.is_null()).then(|| {
            std::ffi::CStr::from_ptr(pointer)
                .to_string_lossy()
                .into_owned()
        })
    };
    io::Error::other(message.unwrap_or_else(|| fallback.to_owned()))
}

impl RuntimeDatabase {
    fn open(path: &str) -> io::Result<Self> {
        if path.is_empty() || path.len() > 32_768 || path.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "database path must be non-empty UTF-8 without NUL",
            ));
        }
        let path = std::ffi::CString::new(path).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "database path contains NUL")
        })?;
        let mut handle = std::ptr::null_mut();
        // SAFETY: all pointers are valid for this call and SQLite initializes handle.
        let code = unsafe {
            sqlite3_open_v2(
                path.as_ptr(),
                &mut handle,
                0x2 | 0x4 | 0x1_0000,
                std::ptr::null(),
            )
        };
        if code != SQLITE_OK {
            let error = database_error(handle, "could not open database");
            if !handle.is_null() {
                // SAFETY: handle came from sqlite3_open_v2 and is not retained.
                unsafe { sqlite3_close_v2(handle) };
            }
            return Err(error);
        }
        // SAFETY: handle is a live SQLite connection and the command is static.
        let configured = unsafe {
            sqlite3_busy_timeout(handle, 5_000) == SQLITE_OK
                && sqlite3_exec(
                    handle,
                    c"PRAGMA foreign_keys=ON".as_ptr(),
                    None,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ) == SQLITE_OK
        };
        if !configured {
            let error = database_error(handle, "could not configure database safety defaults");
            // SAFETY: handle is live and has not been retained.
            unsafe { sqlite3_close_v2(handle) };
            return Err(error);
        }
        Ok(Self(Arc::new(StdMutex::new(RuntimeDatabaseState {
            handle,
            closed: false,
            native: None,
        }))))
    }

    fn native_memory() -> io::Result<Self> {
        Ok(Self(Arc::new(StdMutex::new(RuntimeDatabaseState {
            handle: std::ptr::null_mut(),
            closed: false,
            native: Some(NativeDataStore::default()),
        }))))
    }

    fn with_native<T>(
        &self,
        operation: impl FnOnce(&mut NativeDataStore) -> io::Result<T>,
    ) -> io::Result<Option<T>> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("data store state is poisoned"))?;
        if state.closed {
            return Err(io::Error::other("data store is closed"));
        }
        state.native.as_mut().map(operation).transpose()
    }

    fn is_native(&self) -> io::Result<bool> {
        let state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("data store state is poisoned"))?;
        if state.closed {
            return Err(io::Error::other("data store is closed"));
        }
        Ok(state.native.is_some())
    }

    fn with_state<T>(
        &self,
        operation: impl FnOnce(&mut RuntimeDatabaseState) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("database state is poisoned"))?;
        if state.closed || state.handle.is_null() {
            return Err(io::Error::other("database is closed"));
        }
        operation(&mut state)
    }

    fn prepare(state: &RuntimeDatabaseState, sql: &str) -> io::Result<RuntimeStatement> {
        if sql.is_empty() || sql.len() > DATABASE_SQL_LIMIT || sql.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SQL must be non-empty, at most 1 MiB, and contain no NUL",
            ));
        }
        let bytes = i32::try_from(sql.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "SQL is too large"))?;
        let mut statement = std::ptr::null_mut();
        let mut tail = std::ptr::null();
        // SAFETY: SQL is valid memory for bytes bytes; SQLite returns owned statement/tail.
        let code = unsafe {
            sqlite3_prepare_v2(
                state.handle,
                sql.as_ptr().cast(),
                bytes,
                &mut statement,
                &mut tail,
            )
        };
        if code != SQLITE_OK || statement.is_null() {
            if !statement.is_null() {
                // SAFETY: statement was initialized by SQLite and is not retained.
                unsafe { sqlite3_finalize(statement) };
            }
            return Err(database_error(state.handle, "could not prepare SQL"));
        }
        let start = sql.as_ptr() as usize;
        let tail_offset = (tail as usize)
            .checked_sub(start)
            .filter(|offset| *offset <= sql.len())
            .ok_or_else(|| {
                // SAFETY: statement is live and not retained.
                unsafe { sqlite3_finalize(statement) };
                io::Error::other("SQLite returned an invalid SQL tail")
            })?;
        if tail_offset > sql.len() || !sql[tail_offset..].trim().is_empty() {
            // SAFETY: statement is live and not retained.
            unsafe { sqlite3_finalize(statement) };
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "exactly one SQL statement is allowed",
            ));
        }
        Ok(RuntimeStatement(statement))
    }

    fn bind(
        statement: *mut SqliteStatement,
        parameters: &[RuntimeJson],
    ) -> io::Result<Vec<String>> {
        // SAFETY: statement is live for the duration of this function.
        let expected = unsafe { sqlite3_bind_parameter_count(statement) };
        if expected < 0 || expected as usize != parameters.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SQL parameter count does not match bound values",
            ));
        }
        let mut decoded = Vec::new();
        for (offset, value) in parameters.iter().enumerate() {
            let index = i32::try_from(offset + 1).expect("parameter limit is bounded by SQL size");
            let text = value.text.trim();
            // SAFETY: statement is live and each value pointer remains valid through stepping.
            let code = unsafe {
                match value.kind {
                    "null" => sqlite3_bind_null(statement, index),
                    "bool" => sqlite3_bind_int64(statement, index, i64::from(text == "true")),
                    "number" if text.contains(['.', 'e', 'E']) => {
                        let number = text.parse::<f64>().map_err(|_| {
                            io::Error::new(io::ErrorKind::InvalidInput, "invalid numeric parameter")
                        })?;
                        if !number.is_finite() {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "database parameter is not finite",
                            ));
                        }
                        sqlite3_bind_double(statement, index, number)
                    }
                    "number" => {
                        let number = text.parse::<i64>().map_err(|_| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "database integer parameter does not fit int",
                            )
                        })?;
                        sqlite3_bind_int64(statement, index, number)
                    }
                    "string" => {
                        decoded.push(json_string_value(text)?);
                        let value = decoded.last().expect("just pushed");
                        sqlite3_bind_text(
                            statement,
                            index,
                            value.as_ptr().cast(),
                            i32::try_from(value.len()).map_err(|_| {
                                io::Error::new(
                                    io::ErrorKind::InvalidInput,
                                    "database parameter is too large",
                                )
                            })?,
                            None,
                        )
                    }
                    _ => sqlite3_bind_text(
                        statement,
                        index,
                        text.as_ptr().cast(),
                        i32::try_from(text.len()).map_err(|_| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "database parameter is too large",
                            )
                        })?,
                        None,
                    ),
                }
            };
            if code != SQLITE_OK {
                return Err(io::Error::other("could not bind SQL parameter"));
            }
        }
        Ok(decoded)
    }

    fn execute(&self, sql: &str, parameters: &[RuntimeJson]) -> io::Result<u64> {
        self.with_state(|state| {
            let statement = Self::prepare(state, sql)?;
            let _decoded = Self::bind(statement.0, parameters)?;
            // SAFETY: statement and all bound static data remain live.
            let code = unsafe { sqlite3_step(statement.0) };
            if code == SQLITE_ROW {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "execute cannot discard query rows; use query",
                ));
            }
            if code != SQLITE_DONE {
                return Err(database_error(state.handle, "SQL execution failed"));
            }
            // SAFETY: state owns a live handle.
            Ok(unsafe { sqlite3_changes(state.handle) }.max(0) as u64)
        })
    }

    fn query(&self, sql: &str, parameters: &[RuntimeJson]) -> io::Result<Vec<RuntimeJson>> {
        self.with_state(|state| {
            let statement = Self::prepare(state, sql)?;
            let _decoded = Self::bind(statement.0, parameters)?;
            // SAFETY: statement is live.
            let columns = unsafe { sqlite3_column_count(statement.0) };
            if !(0..=4096).contains(&columns) {
                return Err(io::Error::other("query column count exceeds 4096"));
            }
            let mut rows = Vec::new();
            let mut total = 0usize;
            loop {
                // SAFETY: statement and bound input remain live.
                let code = unsafe { sqlite3_step(statement.0) };
                if code == SQLITE_DONE {
                    break;
                }
                if code != SQLITE_ROW {
                    return Err(database_error(state.handle, "SQL query failed"));
                }
                if rows.len() >= DATABASE_ROW_LIMIT {
                    return Err(io::Error::other("query exceeds the 100000-row limit"));
                }
                let mut names = std::collections::HashSet::new();
                let mut row = String::from("{");
                for column in 0..columns {
                    // SAFETY: column is in range for the current row.
                    let name_pointer = unsafe { sqlite3_column_name(statement.0, column) };
                    if name_pointer.is_null() {
                        return Err(io::Error::other("SQLite column name is unavailable"));
                    }
                    // SAFETY: SQLite returns a NUL-terminated column name.
                    let name = unsafe { std::ffi::CStr::from_ptr(name_pointer) }
                        .to_str()
                        .map_err(|_| io::Error::other("SQLite column name is not valid UTF-8"))?;
                    if !names.insert(name.to_owned()) {
                        return Err(io::Error::other("query contains duplicate column names"));
                    }
                    if column != 0 {
                        row.push(',');
                    }
                    row.push_str(&json_escape_string(name)?);
                    row.push(':');
                    // SAFETY: column is in range for the current row.
                    let kind = unsafe { sqlite3_column_type(statement.0, column) };
                    match kind {
                        SQLITE_NULL => row.push_str("null"),
                        SQLITE_INTEGER => {
                            // SAFETY: SQLite converts the current value to int64.
                            row.push_str(
                                &unsafe { sqlite3_column_int64(statement.0, column) }.to_string(),
                            );
                        }
                        SQLITE_FLOAT => {
                            // SAFETY: SQLite converts the current value to f64.
                            let value = unsafe { sqlite3_column_double(statement.0, column) };
                            if !value.is_finite() {
                                return Err(io::Error::other(
                                    "SQLite floating column is not finite",
                                ));
                            }
                            row.push_str(&value.to_string());
                        }
                        SQLITE_TEXT => {
                            // SAFETY: pointer and length are valid until the next step.
                            let pointer = unsafe { sqlite3_column_text(statement.0, column) };
                            let length = unsafe { sqlite3_column_bytes(statement.0, column) };
                            if length < 0 || (pointer.is_null() && length != 0) {
                                return Err(io::Error::other("could not read SQLite text column"));
                            }
                            let text = if length == 0 {
                                ""
                            } else {
                                // SAFETY: SQLite guarantees length readable bytes here.
                                std::str::from_utf8(unsafe {
                                    std::slice::from_raw_parts(pointer, length as usize)
                                })
                                .map_err(|_| {
                                    io::Error::other("SQLite text column is not valid UTF-8")
                                })?
                            };
                            row.push_str(&json_escape_string(text)?);
                        }
                        SQLITE_BLOB => {
                            return Err(io::Error::other(
                                "SQLite BLOB columns require an explicit byte API",
                            ));
                        }
                        _ => return Err(io::Error::other("unsupported SQLite column type")),
                    }
                }
                row.push('}');
                total = total
                    .checked_add(row.len())
                    .ok_or_else(|| io::Error::other("query output size overflow"))?;
                if total > 16 * 1024 * 1024 {
                    return Err(io::Error::other(
                        "query JSON output exceeds the 16 MiB limit",
                    ));
                }
                rows.push(runtime_json(row)?);
            }
            Ok(rows)
        })
    }

    fn control(&self, sql: &'static [u8], expected_transaction: bool) -> io::Result<()> {
        self.with_state(|state| {
            // SAFETY: state owns a live connection. SQLite returns zero inside a transaction.
            let transaction = unsafe { sqlite3_get_autocommit(state.handle) } == 0;
            if transaction != expected_transaction {
                return Err(io::Error::other(if expected_transaction {
                    "database has no active transaction"
                } else {
                    "database transaction is already active"
                }));
            }
            // SAFETY: sql is a static NUL-terminated command and state is live.
            let code = unsafe {
                sqlite3_exec(
                    state.handle,
                    sql.as_ptr().cast(),
                    None,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if code != SQLITE_OK {
                return Err(database_error(state.handle, "transaction command failed"));
            }
            Ok(())
        })
    }

    fn changes(&self) -> io::Result<u64> {
        self.with_state(|state| {
            // SAFETY: state owns a live handle.
            Ok(unsafe { sqlite3_changes(state.handle) }.max(0) as u64)
        })
    }

    fn last_insert_id(&self) -> io::Result<i64> {
        self.with_state(|state| {
            // SAFETY: state owns a live handle.
            Ok(unsafe { sqlite3_last_insert_rowid(state.handle) })
        })
    }

    fn close(&self) -> io::Result<()> {
        let mut state = self
            .0
            .lock()
            .map_err(|_| io::Error::other("database state is poisoned"))?;
        if state.closed {
            return Ok(());
        }
        if let Some(native) = state.native.as_mut() {
            native.tables.clear();
            state.closed = true;
            return Ok(());
        }
        if state.handle.is_null() {
            state.closed = true;
            return Ok(());
        }
        // SAFETY: state owns a live connection. SQLite returns zero inside a transaction.
        if unsafe { sqlite3_get_autocommit(state.handle) } == 0 {
            // SAFETY: command is static and the handle is live.
            unsafe {
                sqlite3_exec(
                    state.handle,
                    c"ROLLBACK".as_ptr(),
                    None,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
        }
        // SAFETY: no prepared statements escape an operation, so the handle can close.
        let code = unsafe { sqlite3_close_v2(state.handle) };
        if code != SQLITE_OK {
            return Err(database_error(state.handle, "could not close database"));
        }
        state.handle = std::ptr::null_mut();
        state.closed = true;
        Ok(())
    }
}

impl Drop for RuntimeDatabaseState {
    fn drop(&mut self) {
        if self.native.is_some() {
            self.native = None;
            self.closed = true;
            return;
        }
        if self.closed || self.handle.is_null() {
            return;
        }
        // SAFETY: self owns a live connection. SQLite returns zero inside a transaction.
        if unsafe { sqlite3_get_autocommit(self.handle) } == 0 {
            // SAFETY: this is best-effort rollback of a live connection during final cleanup.
            unsafe {
                sqlite3_exec(
                    self.handle,
                    c"ROLLBACK".as_ptr(),
                    None,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
        }
        // SAFETY: RuntimeDatabaseState uniquely owns this handle.
        unsafe { sqlite3_close_v2(self.handle) };
        self.handle = std::ptr::null_mut();
        self.closed = true;
    }
}

fn data_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn data_inner_type(ty: &TypeName) -> (&TypeName, bool) {
    if ty.name == "Option" && ty.arguments.len() == 1 {
        (&ty.arguments[0], true)
    } else {
        (ty, false)
    }
}

fn data_sql_type(ty: &TypeName) -> &'static str {
    match data_inner_type(ty).0.name.as_str() {
        "f32" | "f64" | "float" => "REAL",
        "String" | "char" => "TEXT",
        _ => "INTEGER",
    }
}

fn data_column_sql(field: &crate::ast::FieldDeclaration) -> String {
    let (inner, optional) = data_inner_type(&field.ty);
    let mut sql = format!(
        "{} {}",
        data_identifier(&field.name),
        data_sql_type(&field.ty)
    );
    if !optional {
        sql.push_str(" NOT NULL");
    }
    if inner.name == "bool" {
        let name = data_identifier(&field.name);
        sql.push_str(&format!(" CHECK ({name} IN (0,1))"));
    }
    if field.primary {
        sql.push_str(" PRIMARY KEY");
    }
    sql
}

fn data_create_sql(schema: &crate::ast::StructDeclaration) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {} ({})",
        data_identifier(&schema.name),
        schema
            .fields
            .iter()
            .map(data_column_sql)
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn data_select_sql(schema: &crate::ast::StructDeclaration) -> String {
    schema
        .fields
        .iter()
        .map(|field| data_identifier(&field.name))
        .collect::<Vec<_>>()
        .join(",")
}

fn data_json_integer(value: &RuntimeJson, context: &str) -> io::Result<i64> {
    if value.kind != "number" {
        return Err(io::Error::other(format!("{context} is not an integer")));
    }
    value
        .text
        .parse::<i64>()
        .map_err(|_| io::Error::other(format!("{context} is not an integer")))
}

fn data_ensure_schema(
    database: &RuntimeDatabase,
    schema: &crate::ast::StructDeclaration,
) -> io::Result<()> {
    let fields = schema
        .fields
        .iter()
        .map(|field| NativeDataField {
            name: field.name.clone(),
            storage: data_sql_type(&field.ty),
            optional: data_inner_type(&field.ty).1,
            primary: field.primary,
        })
        .collect::<Vec<_>>();
    if database
        .with_native(|store| {
            if let Some(table) = store.tables.get(&schema.name) {
                if table.fields != fields {
                    return Err(io::Error::other(format!(
                        "stored `{}` layout does not match its DISP Data schema",
                        schema.name
                    )));
                }
                return Ok(());
            }
            let primary = fields
                .iter()
                .position(|field| field.primary)
                .expect("validated data schema has one primary field");
            store.tables.insert(
                schema.name.clone(),
                NativeDataTable {
                    fields,
                    primary,
                    rows: Vec::new(),
                },
            );
            Ok(())
        })?
        .is_some()
    {
        return Ok(());
    }
    database.execute(&data_create_sql(schema), &[])?;
    let pragma = format!("PRAGMA table_info({})", data_identifier(&schema.name));
    let columns = database.query(&pragma, &[])?;
    if columns.len() != schema.fields.len() {
        return Err(io::Error::other(format!(
            "stored `{}` layout does not match its DISP Data schema",
            schema.name
        )));
    }
    for (field, column) in schema.fields.iter().zip(columns) {
        let entries = json_object_entries(&column)?;
        let get = |name: &str| {
            entries
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
                .ok_or_else(|| io::Error::other("storage schema metadata is incomplete"))
        };
        let name = json_string_value(&get("name")?.text)?;
        let storage = json_string_value(&get("type")?.text)?.to_ascii_uppercase();
        let required = data_json_integer(get("notnull")?, "stored nullability")? != 0;
        let primary = data_json_integer(get("pk")?, "stored primary marker")? != 0;
        let optional = data_inner_type(&field.ty).1;
        if name != field.name
            || storage != data_sql_type(&field.ty)
            || required == optional
            || primary != field.primary
        {
            return Err(io::Error::other(format!(
                "stored field `{}` is incompatible with its DISP Data schema",
                field.name
            )));
        }
    }
    Ok(())
}

fn native_data_write(
    database: &RuntimeDatabase,
    schema: &crate::ast::StructDeclaration,
    value: Value,
    replace: bool,
) -> io::Result<Option<u64>> {
    database.with_native(|store| {
        let table = store
            .tables
            .get_mut(&schema.name)
            .ok_or_else(|| io::Error::other("DISP Data schema was not registered"))?;
        let Value::Struct { fields, .. } = &value else {
            return Err(io::Error::other("DISP Data write requires a schema value"));
        };
        let primary_name = &table.fields[table.primary].name;
        let key = fields
            .get(primary_name)
            .ok_or_else(|| io::Error::other("data value is missing its primary field"))?;
        let existing = table.rows.iter().position(|row| {
            matches!(row, Value::Struct { fields, .. } if fields.get(primary_name) == Some(key))
        });
        if let Some(index) = existing {
            if !replace {
                return Err(io::Error::other(format!(
                    "duplicate primary value for `{}`",
                    schema.name
                )));
            }
            table.rows[index] = value;
            return Ok(1);
        }
        if table.rows.len() >= DATABASE_ROW_LIMIT {
            return Err(io::Error::other("data table exceeds the 100000-row limit"));
        }
        table.rows.push(value);
        Ok(1)
    })
}

fn native_data_rows(
    database: &RuntimeDatabase,
    schema: &crate::ast::StructDeclaration,
) -> io::Result<Option<Vec<Value>>> {
    database.with_native(|store| {
        store
            .tables
            .get(&schema.name)
            .map(|table| table.rows.clone())
            .ok_or_else(|| io::Error::other("DISP Data schema was not registered"))
    })
}

fn native_data_replace_rows(
    database: &RuntimeDatabase,
    schema: &crate::ast::StructDeclaration,
    rows: Vec<Value>,
) -> io::Result<Option<()>> {
    database.with_native(|store| {
        let table = store
            .tables
            .get_mut(&schema.name)
            .ok_or_else(|| io::Error::other("DISP Data schema was not registered"))?;
        table.rows = rows;
        Ok(())
    })
}

fn normalize_data_value(ty: &TypeName, value: &RuntimeJson) -> io::Result<RuntimeJson> {
    let (inner, optional) = data_inner_type(ty);
    if optional && value.kind == "null" {
        return Ok(value.clone());
    }
    match inner.name.as_str() {
        "bool" => match data_json_integer(value, "stored bool")? {
            0 => runtime_json("false".into()),
            1 => runtime_json("true".into()),
            _ => Err(io::Error::other("stored bool is outside 0 or 1")),
        },
        "int" | "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" => {
            if value.kind == "number" && !value.text.contains(['.', 'e', 'E']) {
                Ok(value.clone())
            } else {
                Err(io::Error::other("stored data field is not an integer"))
            }
        }
        "f32" | "f64" | "float" if value.kind == "number" => Ok(value.clone()),
        "String" | "char" if value.kind == "string" => Ok(value.clone()),
        _ => Err(io::Error::other("stored data field has an invalid value")),
    }
}

fn data_decode_rows(
    program: &Program,
    schema: &crate::ast::StructDeclaration,
    rows: Vec<RuntimeJson>,
) -> io::Result<Vec<Value>> {
    let ty = TypeName {
        name: schema.name.clone(),
        arguments: vec![],
        qualifier: TypeQualifier::Owned,
        span: schema.name_span,
    };
    rows.into_iter()
        .map(|row| {
            let entries = json_object_entries(&row)?;
            if entries.len() != schema.fields.len() {
                return Err(io::Error::other(
                    "stored row does not match its data schema",
                ));
            }
            let mut normalized = String::from("{");
            for (index, field) in schema.fields.iter().enumerate() {
                let value = entries
                    .iter()
                    .find(|(name, _)| name == &field.name)
                    .map(|(_, value)| value)
                    .ok_or_else(|| io::Error::other("stored row is missing a data field"))?;
                let value = normalize_data_value(&field.ty, value)?;
                if index != 0 {
                    normalized.push(',');
                }
                normalized.push_str(&json_escape_string(&field.name)?);
                normalized.push(':');
                normalized.push_str(&value.text);
            }
            normalized.push('}');
            decode_json_value(program, &ty, &HashMap::new(), &runtime_json(normalized)?)
        })
        .collect()
}

const PROCESS_STREAM_LIMIT: usize = 16 * 1024 * 1024;

#[derive(Default)]
struct RuntimeChildPipe {
    bytes: StdMutex<VecDeque<u8>>,
    done: AtomicBool,
    failed: StdMutex<Option<String>>,
    overflow: AtomicBool,
}

struct RuntimeChildState {
    child: StdChild,
    input: Option<ChildStdin>,
    stdout: Arc<RuntimeChildPipe>,
    stderr: Arc<RuntimeChildPipe>,
    stdout_thread: Option<thread::JoinHandle<()>>,
    stderr_thread: Option<thread::JoinHandle<()>>,
    status: Option<i64>,
    deadline: Option<StdInstant>,
}

struct RuntimeChildInner {
    state: StdMutex<RuntimeChildState>,
}

#[derive(Clone)]
struct RuntimeChildProcess(Arc<RuntimeChildInner>);

impl std::fmt::Debug for RuntimeChildProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_tuple("ChildProcess").finish()
    }
}

impl PartialEq for RuntimeChildProcess {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

fn child_pipe_reader(mut reader: impl Read, pipe: Arc<RuntimeChildPipe>) {
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => {
                let mut bytes = pipe.bytes.lock().expect("child pipe bytes");
                let available = PROCESS_STREAM_LIMIT.saturating_sub(bytes.len());
                let retained = available.min(count);
                if retained < count {
                    pipe.overflow.store(true, Ordering::Release);
                }
                if retained != 0 {
                    bytes.extend(&chunk[..retained]);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                *pipe.failed.lock().expect("child pipe failure") = Some(error.to_string());
                break;
            }
        }
    }
    pipe.done.store(true, Ordering::Release);
}

fn process_status(status: std::process::ExitStatus) -> i64 {
    status.code().map(i64::from).unwrap_or(-1)
}

impl RuntimeChildProcess {
    fn join_readers(&self) -> io::Result<()> {
        let threads = {
            let mut state = self
                .0
                .state
                .lock()
                .map_err(|_| io::Error::other("child-process state is poisoned"))?;
            [state.stdout_thread.take(), state.stderr_thread.take()]
        };
        for thread in threads.into_iter().flatten() {
            thread
                .join()
                .map_err(|_| io::Error::other("child-process reader thread failed"))?;
        }
        Ok(())
    }

    fn check_timeout(state: &mut RuntimeChildState) -> io::Result<()> {
        if state.status.is_none()
            && state
                .deadline
                .is_some_and(|deadline| StdInstant::now() >= deadline)
        {
            let _ = state.child.kill();
            let _ = state.child.wait();
            state.status = Some(124);
            state.input.take();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process exceeded its configured timeout",
            ));
        }
        Ok(())
    }

    fn write(&self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > PROCESS_STREAM_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "process write exceeds the 16 MiB limit",
            ));
        }
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| io::Error::other("child-process state is poisoned"))?;
        Self::check_timeout(&mut state)?;
        let input = state.input.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "child-process input is closed")
        })?;
        input.write_all(bytes)
    }

    fn close_input(&self) -> io::Result<()> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| io::Error::other("child-process state is poisoned"))?;
        Self::check_timeout(&mut state)?;
        state.input.take();
        Ok(())
    }

    fn read_pipe(&self, stdout: bool, limit: usize) -> io::Result<Vec<u8>> {
        if limit > PROCESS_STREAM_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "child-process read limit exceeds 16 MiB",
            ));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        loop {
            let pipe = {
                let mut state = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("child-process state is poisoned"))?;
                Self::check_timeout(&mut state)?;
                if state.status.is_none()
                    && let Some(status) = state.child.try_wait()?
                {
                    state.status = Some(process_status(status));
                    state.input.take();
                }
                if stdout {
                    state.stdout.clone()
                } else {
                    state.stderr.clone()
                }
            };
            if let Some(error) = pipe
                .failed
                .lock()
                .map_err(|_| io::Error::other("child pipe is poisoned"))?
                .clone()
            {
                return Err(io::Error::other(error));
            }
            if pipe.overflow.load(Ordering::Acquire) {
                return Err(io::Error::other(
                    "process output exceeds the 16 MiB capture limit",
                ));
            }
            let mut queued = pipe
                .bytes
                .lock()
                .map_err(|_| io::Error::other("child pipe is poisoned"))?;
            if !queued.is_empty() {
                let count = limit.min(queued.len());
                return Ok(queued.drain(..count).collect());
            }
            if pipe.done.load(Ordering::Acquire) {
                return Ok(Vec::new());
            }
            drop(queued);
            thread::sleep(StdDuration::from_millis(1));
        }
    }

    fn try_wait(&self) -> io::Result<Option<i64>> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| io::Error::other("child-process state is poisoned"))?;
        Self::check_timeout(&mut state)?;
        if state.status.is_none()
            && let Some(status) = state.child.try_wait()?
        {
            state.status = Some(process_status(status));
            state.input.take();
        }
        Ok(state.status)
    }

    fn kill(&self) -> io::Result<()> {
        let mut state = self
            .0
            .state
            .lock()
            .map_err(|_| io::Error::other("child-process state is poisoned"))?;
        if state.status.is_none() {
            state.child.kill()?;
            state.status = Some(process_status(state.child.wait()?));
        }
        state.input.take();
        Ok(())
    }

    fn wait(&self) -> io::Result<Value> {
        let (status, stdout, stderr) = loop {
            let snapshot = {
                let mut state = self
                    .0
                    .state
                    .lock()
                    .map_err(|_| io::Error::other("child-process state is poisoned"))?;
                Self::check_timeout(&mut state)?;
                if state.status.is_none()
                    && let Some(status) = state.child.try_wait()?
                {
                    state.status = Some(process_status(status));
                    state.input.take();
                }
                (state.status, state.stdout.clone(), state.stderr.clone())
            };
            if let Some(status) = snapshot.0
                && snapshot.1.done.load(Ordering::Acquire)
                && snapshot.2.done.load(Ordering::Acquire)
            {
                break (status, snapshot.1, snapshot.2);
            }
            thread::sleep(StdDuration::from_millis(1));
        };
        self.join_readers()?;
        for pipe in [&stdout, &stderr] {
            if let Some(error) = pipe
                .failed
                .lock()
                .map_err(|_| io::Error::other("child pipe is poisoned"))?
                .clone()
            {
                return Err(io::Error::other(error));
            }
            if pipe.overflow.load(Ordering::Acquire) {
                return Err(io::Error::other(
                    "process output exceeds the 16 MiB capture limit",
                ));
            }
        }
        let stdout = stdout
            .bytes
            .lock()
            .map_err(|_| io::Error::other("child pipe is poisoned"))?
            .drain(..)
            .collect();
        let stderr = stderr
            .bytes
            .lock()
            .map_err(|_| io::Error::other("child pipe is poisoned"))?
            .drain(..)
            .collect();
        Ok(Value::ProcessOutput(RuntimeProcessOutput {
            status,
            stdout,
            stderr,
        }))
    }
}

impl Drop for RuntimeChildInner {
    fn drop(&mut self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.status.is_none() {
            let _ = state.child.kill();
            if let Ok(status) = state.child.wait() {
                state.status = Some(process_status(status));
            }
        }
        state.input.take();
        let threads = [state.stdout_thread.take(), state.stderr_thread.take()];
        drop(state);
        for thread in threads.into_iter().flatten() {
            let _ = thread.join();
        }
    }
}

fn start_process(command: RuntimeProcessCommand) -> io::Result<Value> {
    if command.program.as_os_str().is_empty()
        || command
            .program
            .to_str()
            .is_some_and(|value| value.contains('\0'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process program path cannot be empty",
        ));
    }
    let argument_bytes = command
        .arguments
        .iter()
        .try_fold(0usize, |total, argument| total.checked_add(argument.len()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "process arguments overflow"))?;
    if command.arguments.len() > 4096
        || argument_bytes > 1024 * 1024
        || command.arguments.iter().any(|value| value.contains('\0'))
        || command.environment.len() > 4096
        || command.input.len() > PROCESS_STREAM_LIMIT
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process configuration exceeds limits",
        ));
    }
    if command.environment.iter().any(|(name, value)| {
        name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0')
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid process environment override",
        ));
    }
    if command
        .directory
        .as_ref()
        .is_some_and(|directory| directory.to_str().is_some_and(|value| value.contains('\0')))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process working directory cannot contain NUL",
        ));
    }
    let initial_input = command.input;
    let mut configured = StdCommand::new(command.program);
    configured
        .args(command.arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = command.directory {
        configured.current_dir(directory);
    }
    if command.clear_environment {
        configured.env_clear();
    }
    configured.envs(command.environment);
    let mut child = configured.spawn()?;
    let input = child.stdin.take().expect("piped child input");
    let stdout_reader = child.stdout.take().expect("piped child stdout");
    let stderr_reader = child.stderr.take().expect("piped child stderr");
    let stdout = Arc::new(RuntimeChildPipe::default());
    let stderr = Arc::new(RuntimeChildPipe::default());
    let stdout_thread = stdout.clone();
    let stderr_thread = stderr.clone();
    let stdout_thread = thread::spawn(move || child_pipe_reader(stdout_reader, stdout_thread));
    let stderr_thread = thread::spawn(move || child_pipe_reader(stderr_reader, stderr_thread));
    let deadline = command
        .timeout
        .and_then(|timeout| StdInstant::now().checked_add(timeout));
    let process = RuntimeChildProcess(Arc::new(RuntimeChildInner {
        state: StdMutex::new(RuntimeChildState {
            child,
            input: Some(input),
            stdout,
            stderr,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            status: None,
            deadline,
        }),
    }));
    if !initial_input.is_empty() {
        process.write(&initial_input)?;
    }
    Ok(Value::ChildProcess(process))
}

fn execute_process(command: RuntimeProcessCommand) -> io::Result<Value> {
    const MAX_ARGUMENTS: usize = 4096;
    const MAX_ARGUMENT_BYTES: usize = 1024 * 1024;
    const MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
    const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
    if command.program.as_os_str().is_empty()
        || command
            .program
            .to_str()
            .is_some_and(|program| program.contains('\0'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process program path must be non-empty and contain no NUL",
        ));
    }
    if command.arguments.len() > MAX_ARGUMENTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process argument count exceeds 4096",
        ));
    }
    let argument_bytes = command
        .arguments
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(value.len()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "process arguments overflow"))?;
    if argument_bytes > MAX_ARGUMENT_BYTES
        || command.arguments.iter().any(|value| value.contains('\0'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process arguments exceed limits or contain NUL",
        ));
    }
    if command.input.len() > MAX_INPUT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process input exceeds the 16 MiB limit",
        ));
    }
    if command.environment.len() > MAX_ARGUMENTS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process environment override count exceeds 4096",
        ));
    }
    if command.environment.iter().any(|(name, value)| {
        name.is_empty() || name.contains('=') || name.contains('\0') || value.contains('\0')
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process environment names must be non-empty, names cannot contain '=', and names or values cannot contain NUL",
        ));
    }
    if command.directory.as_ref().is_some_and(|directory| {
        directory
            .to_str()
            .is_some_and(|directory| directory.contains('\0'))
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "process working directory cannot contain NUL",
        ));
    }
    let mut child = StdCommand::new(command.program);
    child
        .args(command.arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = command.directory {
        child.current_dir(directory);
    }
    if command.clear_environment {
        child.env_clear();
    }
    child.envs(command.environment);
    let mut child = child.spawn()?;
    let mut stdin = child.stdin.take().expect("piped child stdin");
    let mut stdout = child.stdout.take().expect("piped child stdout");
    let mut stderr = child.stderr.take().expect("piped child stderr");
    let started = StdInstant::now();
    thread::scope(|scope| -> io::Result<Value> {
        let input = command.input;
        let writer = scope.spawn(move || stdin.write_all(&input));
        let out = scope.spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .by_ref()
                .take((MAX_CAPTURE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        });
        let err = scope.spawn(move || {
            let mut bytes = Vec::new();
            stderr
                .by_ref()
                .take((MAX_CAPTURE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        });
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if command
                .timeout
                .is_some_and(|timeout| started.elapsed() >= timeout)
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "process exceeded its configured timeout",
                ));
            }
            thread::sleep(StdDuration::from_millis(1));
        };
        writer
            .join()
            .map_err(|_| io::Error::other("process input writer panicked"))??;
        let stdout = out
            .join()
            .map_err(|_| io::Error::other("process stdout reader panicked"))??;
        let stderr = err
            .join()
            .map_err(|_| io::Error::other("process stderr reader panicked"))??;
        if stdout.len() > MAX_CAPTURE_BYTES || stderr.len() > MAX_CAPTURE_BYTES {
            return Err(io::Error::other(
                "process output exceeds the 16 MiB capture limit",
            ));
        }
        Ok(Value::ProcessOutput(RuntimeProcessOutput {
            status: status.code().map(i64::from).unwrap_or(-1),
            stdout,
            stderr,
        }))
    })
}

#[derive(Debug, Clone)]
struct RuntimeClosure {
    parameters: Vec<crate::ast::Parameter>,
    return_type: Option<crate::ast::TypeName>,
    body: crate::ast::ClosureBody,
    captures: Arc<StdMutex<HashMap<String, Value>>>,
}

impl PartialEq for RuntimeClosure {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.captures, &other.captures)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    scope: usize,
    name: String,
    fields: Vec<PlaceSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlaceSegment {
    Field(String),
    Index(usize),
    MapValue(usize),
    Subslice(usize, usize),
    MutexValue,
}

enum RuntimeFault {
    Error(Diagnostic),
    Propagate(Value),
}

type RuntimeResult<T> = Result<T, RuntimeFault>;
// The recursive semantic oracle retains rich source and ownership values in each
// frame. Keep enough stack for the documented 32-call safety limit on Windows.
const INTERPRETER_STACK_BYTES: usize = 32 * 1024 * 1024;

enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

pub struct Interpreter {
    scopes: Vec<HashMap<String, Value>>,
    scope_orders: Vec<Vec<String>>,
    output: Arc<StdMutex<Vec<String>>>,
    call_depth: usize,
    tasks: Vec<Weak<StdMutex<RuntimeTaskWork>>>,
    http_pool: InterpreterHttpPool,
    program_arguments: Vec<String>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            scopes: Vec::new(),
            scope_orders: Vec::new(),
            output: Arc::new(StdMutex::new(Vec::new())),
            call_depth: 0,
            tasks: Vec::new(),
            http_pool: HashMap::new(),
            program_arguments: Vec::new(),
        }
    }

    pub fn run(&mut self, program: &Program) -> Result<Vec<String>, Diagnostic> {
        self.program_arguments.clear();
        self.run_configured(program)
    }

    fn run_configured(&mut self, program: &Program) -> Result<Vec<String>, Diagnostic> {
        thread::scope(|scope| {
            let worker = thread::Builder::new()
                .name("disp-interpreter".into())
                .stack_size(INTERPRETER_STACK_BYTES)
                .spawn_scoped(scope, || self.run_inner(program))
                .map_err(|error| {
                    Diagnostic::new(
                        DiagnosticKind::Runtime,
                        format!("could not start interpreter: {error}"),
                        Span::point(1, 1),
                    )
                })?;
            worker.join().map_err(|_| {
                Diagnostic::new(
                    DiagnosticKind::Runtime,
                    "interpreter worker panicked",
                    Span::point(1, 1),
                )
            })?
        })
    }

    pub fn run_with_args(
        &mut self,
        program: &Program,
        arguments: &[String],
    ) -> Result<Vec<String>, Diagnostic> {
        self.program_arguments = arguments.to_vec();
        self.run_configured(program)
    }

    fn run_inner(&mut self, program: &Program) -> Result<Vec<String>, Diagnostic> {
        self.scopes.clear();
        self.scope_orders.clear();
        self.output
            .lock()
            .expect("interpreter output lock poisoned")
            .clear();
        self.call_depth = 0;
        self.tasks.clear();
        self.http_pool.clear();
        let main = program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .ok_or_else(|| self.diagnostic("missing `main` function", Span::point(1, 1)))?
            .clone();
        let arguments = if main.parameters.is_empty() {
            Vec::new()
        } else {
            let values = self
                .program_arguments
                .iter()
                .cloned()
                .map(|value| Value::String(RuntimeString::literal(value)))
                .collect::<Vec<_>>();
            vec![Value::List {
                capacity: values.len(),
                values,
            }]
        };
        let result = self
            .call_function(program, &main, arguments, main.name_span)
            .map_err(RuntimeFault::into_diagnostic)?;
        if let Value::Future(future) = result {
            self.await_future(program, future, main.name_span)
                .map_err(RuntimeFault::into_diagnostic)?;
        }
        Ok(std::mem::take(
            &mut *self
                .output
                .lock()
                .expect("interpreter output lock poisoned"),
        ))
    }

    fn call_function(
        &mut self,
        program: &Program,
        function: &Function,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if function.asynchronous {
            return Ok(Value::Future(RuntimeFuture::new(
                function.clone(),
                arguments,
            )));
        }
        self.call_function_body(program, function, arguments, call_span)
    }

    fn call_function_body(
        &mut self,
        program: &Program,
        function: &Function,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if function.external.is_some() {
            return self.call_external(function, arguments, call_span);
        }
        // Keep the semantic oracle below the smallest supported test-thread stack.
        // Native DISP programs use the platform stack and are not capped here.
        const MAX_CALL_DEPTH: usize = 32;
        if function.parameters.len() != arguments.len() {
            return Err(self.error(
                format!(
                    "function `{}` expects {} arguments, found {}",
                    function.name,
                    function.parameters.len(),
                    arguments.len()
                ),
                call_span,
            ));
        }
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(self.error(
                format!("call depth exceeds the runtime limit of {MAX_CALL_DEPTH}"),
                call_span,
            ));
        }
        self.call_depth += 1;
        self.push_scope(HashMap::new());
        for (parameter, value) in function.parameters.iter().zip(arguments) {
            let value = coerce_value(value, &parameter.ty)
                .map_err(|message| self.error(message, call_span))?;
            self.scopes
                .last_mut()
                .unwrap()
                .insert(parameter.name.clone(), value);
            self.scope_orders
                .last_mut()
                .unwrap()
                .push(parameter.name.clone());
        }
        let flow = self.execute_block_contents(program, &function.body);
        self.pop_scope();
        self.call_depth -= 1;
        match flow {
            Err(RuntimeFault::Propagate(value)) => Ok(value),
            Err(error) => Err(error),
            Ok(Flow::Return(value)) => {
                if let Some(return_type) = &function.return_type {
                    coerce_value(value, return_type)
                        .map_err(|message| self.error(message, call_span))
                } else {
                    Ok(value)
                }
            }
            Ok(Flow::Normal) => Ok(Value::Unit),
            Ok(Flow::Break | Flow::Continue) => {
                Err(self.error("loop control escaped a function body", function.body.span))
            }
        }
    }

    fn await_future(
        &mut self,
        program: &Program,
        future: RuntimeFuture,
        span: Span,
    ) -> RuntimeResult<Value> {
        let work = future
            .0
            .lock()
            .map_err(|_| self.error("future state is poisoned", span))?
            .take()
            .ok_or_else(|| self.error("future has already been awaited", span))?;
        match work {
            FutureWork::Function(function, arguments) => {
                self.call_function_body(program, &function, arguments, span)
            }
            FutureWork::Yield => {
                self.progress_tasks(program, span)?;
                Ok(Value::Unit)
            }
            FutureWork::Sleep(duration) => {
                thread::sleep(duration);
                Ok(Value::Unit)
            }
            FutureWork::ReadText(path) => Ok(runtime_result(
                fs::read_to_string(path).map(|text| Value::String(RuntimeString::literal(text))),
            )),
            FutureWork::ReadBytes(path) => Ok(runtime_result(fs::read(path).map(|bytes| {
                let values = bytes
                    .into_iter()
                    .map(|value| Value::Unsigned(value as u128, 8))
                    .collect::<Vec<_>>();
                Value::List {
                    capacity: values.len(),
                    values,
                }
            }))),
            FutureWork::WriteText(path, text) => Ok(runtime_result(
                fs::write(path, text.text).map(|()| Value::Unit),
            )),
            FutureWork::WriteBytes(path, bytes) => {
                Ok(runtime_result(fs::write(path, bytes).map(|()| Value::Unit)))
            }
            FutureWork::Connect(address, timeout) => {
                let connected = if let Some(timeout) = timeout {
                    match (address.host.as_str(), address.port).to_socket_addrs() {
                        Ok(addresses) => {
                            let mut last = None;
                            let mut connected = None;
                            for address in addresses {
                                match StdTcpStream::connect_timeout(&address, timeout) {
                                    Ok(stream) => {
                                        connected = Some(stream);
                                        break;
                                    }
                                    Err(error) => last = Some(error),
                                }
                            }
                            connected.ok_or_else(|| {
                                last.unwrap_or_else(|| {
                                    std::io::Error::new(
                                        std::io::ErrorKind::AddrNotAvailable,
                                        "address resolution returned no addresses",
                                    )
                                })
                            })
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    StdTcpStream::connect((address.host.as_str(), address.port))
                };
                Ok(runtime_result(connected.map(|stream| {
                    Value::TcpStream(RuntimeTcpStream::new(stream))
                })))
            }
            FutureWork::Accept(listener, timeout) => {
                let deadline = timeout.and_then(|duration| StdInstant::now().checked_add(duration));
                loop {
                    let accepted = {
                        let guard = listener
                            .0
                            .lock()
                            .map_err(|_| self.error("TCP listener state is poisoned", span))?;
                        match guard.as_ref() {
                            Some(listener) => listener.accept(),
                            None => Err(std::io::Error::new(
                                std::io::ErrorKind::NotConnected,
                                "TCP listener is closed",
                            )),
                        }
                    };
                    match accepted {
                        Ok((stream, _)) => {
                            break Ok(runtime_result(Ok(Value::TcpStream(RuntimeTcpStream::new(
                                stream,
                            )))));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if deadline.is_some_and(|deadline| StdInstant::now() >= deadline) {
                                break Ok(runtime_result(Err(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "TCP accept timed out",
                                ))));
                            }
                            thread::sleep(StdDuration::from_millis(1));
                        }
                        Err(error) => break Ok(runtime_result(Err(error))),
                    }
                }
            }
            FutureWork::SocketRead(stream, limit, timeout) => {
                let mut guard = stream
                    .0
                    .lock()
                    .map_err(|_| self.error("TCP stream state is poisoned", span))?;
                let result = if guard.read_shutdown {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "TCP read side is shut down",
                    ))
                } else if let Some(socket) = guard.socket.as_mut() {
                    socket.set_read_timeout(timeout).and_then(|()| {
                        let mut bytes = vec![0; limit];
                        let result = socket.read(&mut bytes).map(|count| {
                            bytes.truncate(count);
                            Value::List {
                                capacity: bytes.len(),
                                values: bytes
                                    .into_iter()
                                    .map(|byte| Value::Unsigned(byte as u128, 8))
                                    .collect(),
                            }
                        });
                        let _ = socket.set_read_timeout(None);
                        result
                    })
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "TCP stream is closed",
                    ))
                };
                Ok(runtime_result(result))
            }
            FutureWork::SocketWrite(stream, bytes, timeout) => {
                let mut guard = stream
                    .0
                    .lock()
                    .map_err(|_| self.error("TCP stream state is poisoned", span))?;
                let result = if guard.write_shutdown {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "TCP write side is shut down",
                    ))
                } else if let Some(socket) = guard.socket.as_mut() {
                    socket.set_write_timeout(timeout).and_then(|()| {
                        let result = socket
                            .write_all(&bytes)
                            .map(|()| Value::UInt(bytes.len() as u64));
                        let _ = socket.set_write_timeout(None);
                        result
                    })
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "TCP stream is closed",
                    ))
                };
                Ok(runtime_result(result))
            }
            FutureWork::UdpReceive(socket, limit, timeout) => {
                let mut guard = socket
                    .0
                    .lock()
                    .map_err(|_| self.error("UDP socket state is poisoned", span))?;
                let result = if let Some(socket) = guard.as_mut() {
                    socket.set_read_timeout(timeout).and_then(|()| {
                        let mut bytes = vec![0; limit.saturating_add(1)];
                        let result = socket.recv_from(&mut bytes).and_then(|(count, source)| {
                            if count > limit {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::InvalidData,
                                    "UDP datagram exceeds receive limit",
                                ));
                            }
                            bytes.truncate(count);
                            Ok(Value::UdpDatagram(RuntimeUdpDatagram {
                                source: RuntimeSocketAddress {
                                    host: source.ip().to_string(),
                                    port: source.port(),
                                },
                                bytes,
                            }))
                        });
                        let _ = socket.set_read_timeout(None);
                        result
                    })
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "UDP socket is closed",
                    ))
                };
                Ok(runtime_result(result))
            }
            FutureWork::UdpSend(socket, bytes, address, timeout) => {
                let mut guard = socket
                    .0
                    .lock()
                    .map_err(|_| self.error("UDP socket state is poisoned", span))?;
                let result = if bytes.len() > 65_507 {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "UDP datagram exceeds the 65507-byte payload limit",
                    ))
                } else if let Some(socket) = guard.as_mut() {
                    socket.set_write_timeout(timeout).and_then(|()| {
                        let result = socket
                            .send_to(&bytes, (address.host.as_str(), address.port))
                            .map(|count| Value::UInt(count as u64));
                        let _ = socket.set_write_timeout(None);
                        result
                    })
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "UDP socket is closed",
                    ))
                };
                Ok(runtime_result(result))
            }
            FutureWork::Resolve(host, timeout) => {
                if timeout.is_some_and(|timeout| timeout.is_zero()) {
                    return Ok(runtime_result(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "DNS resolution timed out",
                    ))));
                }
                let started = StdInstant::now();
                let resolved = resolve_ip_addresses(&host).and_then(|addresses| {
                    if timeout.is_some_and(|timeout| started.elapsed() > timeout) {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "DNS resolution timed out",
                        ));
                    }
                    let values = addresses
                        .into_iter()
                        .map(|address| Value::IpAddress(RuntimeIpAddress(address)))
                        .collect::<Vec<_>>();
                    Ok(Value::List {
                        capacity: values.len(),
                        values,
                    })
                });
                Ok(runtime_result(resolved))
            }
            FutureWork::TlsConnect(stream, server_name, timeout) => {
                if server_name.is_empty() || server_name.contains('\0') {
                    return Ok(runtime_result(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "TLS server name must not be empty or contain NUL",
                    ))));
                }
                if timeout.is_some_and(|duration| duration.is_zero()) {
                    return Ok(runtime_result(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "TLS handshake timed out",
                    ))));
                }
                let socket = {
                    let mut state = stream
                        .0
                        .lock()
                        .map_err(|_| self.error("TCP stream state is poisoned", span))?;
                    state.socket.take()
                };
                let Some(socket) = socket else {
                    return Ok(runtime_result(Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "TCP stream is closed",
                    ))));
                };
                let result = (|| {
                    socket.set_nonblocking(false)?;
                    socket.set_read_timeout(timeout)?;
                    socket.set_write_timeout(timeout)?;
                    let mut builder = TlsConnector::builder();
                    builder.min_protocol_version(Some(Protocol::Tlsv12));
                    let connector = builder.build().map_err(tls_error)?;
                    let secure = connector.connect(&server_name, socket).map_err(tls_error)?;
                    secure.get_ref().set_read_timeout(None)?;
                    secure.get_ref().set_write_timeout(None)?;
                    Ok(Value::TlsStream(RuntimeTlsStream(Arc::new(StdMutex::new(
                        Some(secure),
                    )))))
                })();
                Ok(runtime_result(result))
            }
            FutureWork::TlsRead(stream, limit, timeout) => {
                let mut guard = stream
                    .0
                    .lock()
                    .map_err(|_| self.error("TLS stream state is poisoned", span))?;
                let result = if let Some(stream) = guard.as_mut() {
                    stream.get_ref().set_read_timeout(timeout).and_then(|()| {
                        let mut bytes = vec![0; limit];
                        let result = stream.read(&mut bytes).map(|count| {
                            bytes.truncate(count);
                            runtime_bytes(bytes)
                        });
                        let _ = stream.get_ref().set_read_timeout(None);
                        result
                    })
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "TLS stream is closed",
                    ))
                };
                Ok(runtime_result(result))
            }
            FutureWork::TlsWrite(stream, bytes, timeout) => {
                let mut guard = stream
                    .0
                    .lock()
                    .map_err(|_| self.error("TLS stream state is poisoned", span))?;
                let result = if let Some(stream) = guard.as_mut() {
                    stream.get_ref().set_write_timeout(timeout).and_then(|()| {
                        let result = stream
                            .write_all(&bytes)
                            .map(|()| Value::UInt(bytes.len() as u64));
                        let _ = stream.get_ref().set_write_timeout(None);
                        result
                    })
                } else {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::NotConnected,
                        "TLS stream is closed",
                    ))
                };
                Ok(runtime_result(result))
            }
            FutureWork::HttpRequest(request, timeout) => Ok(runtime_result(
                interpreter_http_request(request, timeout, &mut self.http_pool),
            )),
        }
    }

    fn progress_tasks(&mut self, program: &Program, span: Span) -> RuntimeResult<()> {
        self.tasks.retain(|task| task.strong_count() > 0);
        let tasks = self.tasks.clone();
        for weak in tasks {
            let Some(state) = weak.upgrade() else {
                continue;
            };
            let work = {
                let mut work = state
                    .lock()
                    .map_err(|_| self.error("task state is poisoned", span))?;
                match &*work {
                    RuntimeTaskWork::Future(_) => {
                        std::mem::replace(&mut *work, RuntimeTaskWork::Running)
                    }
                    RuntimeTaskWork::Running
                    | RuntimeTaskWork::Ready(_)
                    | RuntimeTaskWork::Consumed => continue,
                }
            };
            let RuntimeTaskWork::Future(future) = work else {
                unreachable!()
            };
            match self.await_future(program, future, span) {
                Ok(value) => {
                    *state
                        .lock()
                        .map_err(|_| self.error("task state is poisoned", span))? =
                        RuntimeTaskWork::Ready(value);
                }
                Err(error) => {
                    if let Ok(mut work) = state.lock() {
                        *work = RuntimeTaskWork::Consumed;
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn call_closure(
        &mut self,
        program: &Program,
        closure: RuntimeClosure,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if closure.parameters.len() != arguments.len() {
            return Err(self.error(
                format!(
                    "closure expects {} arguments, found {}",
                    closure.parameters.len(),
                    arguments.len()
                ),
                call_span,
            ));
        }
        const MAX_CALL_DEPTH: usize = 32;
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(self.error(
                format!("call depth exceeds the runtime limit of {MAX_CALL_DEPTH}"),
                call_span,
            ));
        }
        self.call_depth += 1;
        let captures = closure
            .captures
            .lock()
            .map_err(|_| self.error("closure capture state is poisoned", call_span))?
            .clone();
        self.push_scope(captures);
        self.push_scope(HashMap::new());
        for (parameter, value) in closure.parameters.iter().zip(arguments) {
            let value = coerce_value(value, &parameter.ty)
                .map_err(|message| self.error(message, call_span))?;
            self.scopes
                .last_mut()
                .unwrap()
                .insert(parameter.name.clone(), value);
            self.scope_orders
                .last_mut()
                .unwrap()
                .push(parameter.name.clone());
        }
        let flow = match &closure.body {
            crate::ast::ClosureBody::Expression(value) => {
                self.evaluate(program, value).map(Flow::Return)
            }
            crate::ast::ClosureBody::Block(block) => self.execute_block_contents(program, block),
        };
        self.pop_scope();
        let captures = self.scopes.pop().unwrap_or_default();
        self.scope_orders.pop();
        *closure
            .captures
            .lock()
            .map_err(|_| self.error("closure capture state is poisoned", call_span))? = captures;
        self.call_depth -= 1;
        let value = match flow {
            Err(RuntimeFault::Propagate(value)) => value,
            Err(error) => return Err(error),
            Ok(Flow::Return(value)) => value,
            Ok(Flow::Normal) => Value::Unit,
            Ok(Flow::Break | Flow::Continue) => {
                return Err(self.error("loop control escaped a closure body", call_span));
            }
        };
        if let Some(return_type) = &closure.return_type {
            coerce_value(value, return_type).map_err(|message| self.error(message, call_span))
        } else {
            Ok(value)
        }
    }

    fn call_external(
        &mut self,
        function: &Function,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if function.parameters.len() != arguments.len() {
            return Err(self.error(
                format!(
                    "external function `{}` expects {} arguments, found {}",
                    function.name,
                    function.parameters.len(),
                    arguments.len()
                ),
                call_span,
            ));
        }
        let link_name = function
            .external
            .as_ref()
            .map_or(function.name.as_str(), |external| {
                external.link_name.as_str()
            });
        match link_name {
            "abs" => {
                let value = match arguments.into_iter().next().unwrap() {
                    Value::Signed(value, 32) => i32::try_from(value).ok(),
                    Value::Int(value) => i32::try_from(value).ok(),
                    _ => None,
                }
                .ok_or_else(|| self.error("C abs expects CInt", call_span))?;
                let value = value
                    .checked_abs()
                    .ok_or_else(|| self.error("C abs overflow", call_span))?;
                Ok(Value::Signed(value.into(), 32))
            }
            "strlen" => {
                let value = match arguments.into_iter().next().unwrap() {
                    Value::CStr(value) | Value::CString(value) => value,
                    _ => return Err(self.error("C strlen expects CStr", call_span)),
                };
                Ok(Value::UInt(value.len() as u64))
            }
            "sqrt" => {
                let value = arguments.into_iter().next().unwrap();
                let value = numeric_as_f64(&value)
                    .ok_or_else(|| self.error("C sqrt expects CDouble", call_span))?;
                Ok(Value::Float(value.sqrt()))
            }
            name => Err(self.error(
                format!("external C function `{name}` requires native execution"),
                call_span,
            )),
        }
    }

    fn execute_block(&mut self, program: &Program, block: &Block) -> RuntimeResult<Flow> {
        self.push_scope(HashMap::new());
        let result = self.execute_block_contents(program, block);
        self.pop_scope();
        result
    }

    fn execute_block_contents(&mut self, program: &Program, block: &Block) -> RuntimeResult<Flow> {
        for statement in &block.statements {
            let flow = self.execute_statement(program, &statement.node, statement.span)?;
            if !matches!(flow, Flow::Normal) {
                return Ok(flow);
            }
        }
        Ok(Flow::Normal)
    }

    fn execute_statement(
        &mut self,
        program: &Program,
        statement: &Statement,
        span: Span,
    ) -> RuntimeResult<Flow> {
        match statement {
            Statement::Binding {
                name,
                annotation,
                value,
                ..
            } => {
                let mut value = value
                    .as_ref()
                    .map(|value| self.consume(program, value))
                    .transpose()?
                    .unwrap_or(Value::Uninitialized);
                if let Some(annotation) = annotation
                    && !matches!(value, Value::Uninitialized)
                {
                    value = coerce_value(value, annotation)
                        .map_err(|message| self.error(message, span))?;
                }
                self.scopes.last_mut().unwrap().insert(name.clone(), value);
                self.scope_orders.last_mut().unwrap().push(name.clone());
                Ok(Flow::Normal)
            }
            Statement::Assignment {
                name,
                operator,
                value,
                ..
            } => {
                let mut right = self.consume(program, value)?;
                if let Some(existing) = self.lookup(name).and_then(|value| match value {
                    Value::CaptureReference(place, _) => self.read_place(&place),
                    value => Some(value),
                }) {
                    right = coerce_like(right, &existing)
                        .map_err(|message| self.error(message, span))?;
                }
                let new_value = if *operator == AssignmentOperator::Assign {
                    right
                } else {
                    let left = self
                        .lookup(name)
                        .and_then(|value| match value {
                            Value::CaptureReference(place, _) => self.read_place(&place),
                            value => Some(value),
                        })
                        .ok_or_else(|| self.error(format!("undefined variable `{name}`"), span))?;
                    let binary = match operator {
                        AssignmentOperator::Add => BinaryOperator::Add,
                        AssignmentOperator::Subtract => BinaryOperator::Subtract,
                        AssignmentOperator::Multiply => BinaryOperator::Multiply,
                        AssignmentOperator::Divide => BinaryOperator::Divide,
                        AssignmentOperator::Assign => unreachable!(),
                    };
                    self.evaluate_binary(binary, left, right, span)?
                };
                if self.assign(name, new_value.clone()).is_none() {
                    if *operator != AssignmentOperator::Assign {
                        return Err(self.error(format!("undefined variable `{name}`"), span));
                    }
                    self.scopes
                        .last_mut()
                        .unwrap()
                        .insert(name.clone(), new_value);
                    self.scope_orders.last_mut().unwrap().push(name.clone());
                }
                Ok(Flow::Normal)
            }
            Statement::PlaceAssignment {
                target,
                operator,
                value,
            } => {
                let place = self
                    .dynamic_place(program, target)?
                    .ok_or_else(|| self.error("assignment target is not a place", target.span))?;
                let right = self.consume(program, value)?;
                let new_value = if *operator == AssignmentOperator::Assign {
                    right
                } else {
                    let left = self
                        .read_place(&place)
                        .ok_or_else(|| self.error("invalid assignment target", target.span))?;
                    self.evaluate_binary(
                        match operator {
                            AssignmentOperator::Add => BinaryOperator::Add,
                            AssignmentOperator::Subtract => BinaryOperator::Subtract,
                            AssignmentOperator::Multiply => BinaryOperator::Multiply,
                            AssignmentOperator::Divide => BinaryOperator::Divide,
                            AssignmentOperator::Assign => unreachable!(),
                        },
                        left,
                        right,
                        span,
                    )?
                };
                self.write_place(&place, new_value)
                    .ok_or_else(|| self.error("invalid assignment target", target.span))?;
                Ok(Flow::Normal)
            }
            Statement::Expression(expression) => {
                self.evaluate(program, expression)?;
                Ok(Flow::Normal)
            }
            Statement::Return(value) => {
                let value = value
                    .as_ref()
                    .map(|expression| self.consume(program, expression))
                    .transpose()?
                    .unwrap_or(Value::Unit);
                Ok(Flow::Return(value))
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => match self.evaluate(program, condition)? {
                Value::Bool(true) => self.execute_block(program, then_branch),
                Value::Bool(false) => {
                    if let Some(branch) = else_branch {
                        self.execute_block(program, branch)
                    } else {
                        Ok(Flow::Normal)
                    }
                }
                _ => Err(self.error("if condition did not evaluate to Bool", condition.span)),
            },
            Statement::While { condition, body } => {
                loop {
                    match self.evaluate(program, condition)? {
                        Value::Bool(true) => {}
                        Value::Bool(false) => break,
                        _ => {
                            return Err(self.error(
                                "while condition did not evaluate to Bool",
                                condition.span,
                            ));
                        }
                    }
                    match self.execute_block(program, body)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::For {
                name,
                start,
                end,
                inclusive,
                body,
                ..
            } => {
                let Value::Int(mut current) = self.evaluate(program, start)? else {
                    return Err(self.error("range start did not evaluate to int", start.span));
                };
                let Value::Int(end) = self.evaluate(program, end)? else {
                    return Err(self.error("range end did not evaluate to int", end.span));
                };
                while if *inclusive {
                    current <= end
                } else {
                    current < end
                } {
                    self.push_scope(HashMap::from([(name.clone(), Value::Int(current))]));
                    let flow = self.execute_block_contents(program, body);
                    self.pop_scope();
                    match flow? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                    if *inclusive && current == end {
                        break;
                    }
                    current = current.checked_add(1).ok_or_else(|| {
                        self.error("integer overflow while advancing for-loop range", span)
                    })?;
                }
                Ok(Flow::Normal)
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                let iterable = match self.evaluate(program, iterable)? {
                    Value::Reference(place, _) => self
                        .read_place(&place)
                        .ok_or_else(|| self.error("dangling iterable reference", iterable.span))?,
                    value => value,
                };
                let values = match iterable {
                    Value::Array(values) | Value::Slice(values) => values,
                    Value::List { values, .. } | Value::Set { values, .. } => values,
                    _ => {
                        return Err(self.error("iteration requires an array, slice, or List", span));
                    }
                };
                for value in values {
                    self.push_scope(HashMap::from([(name.clone(), value)]));
                    let flow = self.execute_block_contents(program, body);
                    self.pop_scope();
                    match flow? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::Loop(body) => {
                loop {
                    match self.execute_block(program, body)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::Unsafe(body) => self.execute_block(program, body),
            Statement::Break => Ok(Flow::Break),
            Statement::Continue => Ok(Flow::Continue),
        }
    }

    fn data_expression_sql(
        &mut self,
        program: &Program,
        schema: &crate::ast::StructDeclaration,
        expression: &Expr,
        parameters: &mut Vec<RuntimeJson>,
    ) -> RuntimeResult<String> {
        match &expression.node {
            Expression::Identifier(name)
                if schema.fields.iter().any(|field| field.name == *name)
                    && self.expression_place(expression).is_none() =>
            {
                Ok(data_identifier(name))
            }
            Expression::Unary { operator, operand } => {
                let operand = self.data_expression_sql(program, schema, operand, parameters)?;
                Ok(match operator {
                    UnaryOperator::Not => format!("(NOT {operand})"),
                    UnaryOperator::Negate => format!("(-{operand})"),
                })
            }
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.data_expression_sql(program, schema, left, parameters)?;
                let right = self.data_expression_sql(program, schema, right, parameters)?;
                let operator = match operator {
                    BinaryOperator::Add => "+",
                    BinaryOperator::Subtract => "-",
                    BinaryOperator::Multiply => "*",
                    BinaryOperator::Divide => "/",
                    BinaryOperator::Remainder => "%",
                    BinaryOperator::Equal => "=",
                    BinaryOperator::NotEqual => "<>",
                    BinaryOperator::Less => "<",
                    BinaryOperator::LessEqual => "<=",
                    BinaryOperator::Greater => ">",
                    BinaryOperator::GreaterEqual => ">=",
                    BinaryOperator::And => "AND",
                    BinaryOperator::Or => "OR",
                };
                Ok(format!("({left} {operator} {right})"))
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_)
            | Expression::Identifier(_) => {
                let value = self.evaluate(program, expression)?;
                parameters.push(
                    encode_json_value(program, &value)
                        .map_err(|cause| self.error(cause.to_string(), expression.span))?,
                );
                Ok("?".into())
            }
            _ => Err(self.error(
                "unsupported expression reached a DISP Data plan",
                expression.span,
            )),
        }
    }

    fn evaluate_data_expression(
        &mut self,
        program: &Program,
        schema: &crate::ast::StructDeclaration,
        row: &HashMap<String, Value>,
        parameters: &HashMap<Span, Value>,
        expression: &Expr,
    ) -> RuntimeResult<Value> {
        match &expression.node {
            Expression::Identifier(name)
                if schema.fields.iter().any(|field| field.name == *name)
                    && self.expression_place(expression).is_none() =>
            {
                row.get(name).cloned().ok_or_else(|| {
                    self.error("stored row is missing a DISP Data field", expression.span)
                })
            }
            Expression::Unary { operator, operand } => {
                let value =
                    self.evaluate_data_expression(program, schema, row, parameters, operand)?;
                match (operator, value) {
                    (UnaryOperator::Negate, Value::Int(value)) => {
                        value.checked_neg().map(Value::Int).ok_or_else(|| {
                            self.error("integer overflow in DISP Data negation", expression.span)
                        })
                    }
                    (UnaryOperator::Negate, Value::Signed(value, width)) => value
                        .checked_neg()
                        .filter(|value| {
                            width == 128
                                || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1)
                                    .contains(value)
                        })
                        .map(|value| Value::Signed(value, width))
                        .ok_or_else(|| {
                            self.error("integer overflow in DISP Data negation", expression.span)
                        }),
                    (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                    (UnaryOperator::Negate, Value::Float32(value)) => Ok(Value::Float32(-value)),
                    (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                    _ => Err(self.error("invalid DISP Data unary operand", expression.span)),
                }
            }
            Expression::Binary {
                left,
                operator: BinaryOperator::And,
                right,
            } => match self.evaluate_data_expression(program, schema, row, parameters, left)? {
                Value::Bool(false) => Ok(Value::Bool(false)),
                Value::Bool(true) => {
                    match self.evaluate_data_expression(program, schema, row, parameters, right)? {
                        Value::Bool(value) => Ok(Value::Bool(value)),
                        _ => Err(self.error("right DISP Data operand is not bool", right.span)),
                    }
                }
                _ => Err(self.error("left DISP Data operand is not bool", left.span)),
            },
            Expression::Binary {
                left,
                operator: BinaryOperator::Or,
                right,
            } => match self.evaluate_data_expression(program, schema, row, parameters, left)? {
                Value::Bool(true) => Ok(Value::Bool(true)),
                Value::Bool(false) => {
                    match self.evaluate_data_expression(program, schema, row, parameters, right)? {
                        Value::Bool(value) => Ok(Value::Bool(value)),
                        _ => Err(self.error("right DISP Data operand is not bool", right.span)),
                    }
                }
                _ => Err(self.error("left DISP Data operand is not bool", left.span)),
            },
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate_data_expression(program, schema, row, parameters, left)?;
                let right_span = right.span;
                let right =
                    self.evaluate_data_expression(program, schema, row, parameters, right)?;
                let (left, right) = coerce_numeric_pair(left, right)
                    .map_err(|message| self.error(message, right_span))?;
                self.evaluate_binary(*operator, left, right, expression.span)
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => self.evaluate(program, expression),
            Expression::Identifier(_) => {
                parameters.get(&expression.span).cloned().ok_or_else(|| {
                    self.error("DISP Data parameter was not evaluated", expression.span)
                })
            }
            _ => Err(self.error(
                "unsupported expression reached a native DISP Data plan",
                expression.span,
            )),
        }
    }

    fn capture_data_parameters(
        &mut self,
        program: &Program,
        schema: &crate::ast::StructDeclaration,
        expression: &Expr,
        output: &mut HashMap<Span, Value>,
    ) -> RuntimeResult<()> {
        match &expression.node {
            Expression::Identifier(name)
                if schema.fields.iter().any(|field| field.name == *name)
                    && self.expression_place(expression).is_none() =>
            {
                Ok(())
            }
            Expression::Identifier(_) => {
                output.insert(expression.span, self.evaluate(program, expression)?);
                Ok(())
            }
            Expression::Unary { operand, .. } => {
                self.capture_data_parameters(program, schema, operand, output)
            }
            Expression::Binary { left, right, .. } => {
                self.capture_data_parameters(program, schema, left, output)?;
                self.capture_data_parameters(program, schema, right, output)
            }
            Expression::Integer(_)
            | Expression::Float(_)
            | Expression::String(_)
            | Expression::Character(_)
            | Expression::Bool(_) => Ok(()),
            _ => Err(self.error(
                "unsupported expression reached a native DISP Data plan",
                expression.span,
            )),
        }
    }

    fn evaluate_data_write(
        &mut self,
        program: &Program,
        value: &Expr,
        store: &Expr,
        replace: bool,
        span: Span,
    ) -> RuntimeResult<Value> {
        let Value::Database(database) = self.evaluate(program, store)? else {
            unreachable!("type checking validates the DISP Data store")
        };
        let value = self.evaluate(program, value)?;
        let Value::Struct { type_name, .. } = &value else {
            unreachable!("type checking validates the DISP Data value")
        };
        let schema = program
            .structs
            .iter()
            .find(|schema| schema.name == *type_name && schema.data)
            .expect("type checking validates the DISP Data schema");
        if let Err(error) = data_ensure_schema(&database, schema) {
            return Ok(runtime_result(Err(error)));
        }
        if database
            .is_native()
            .map_err(|error| self.error(error.to_string(), store.span))?
        {
            let result = native_data_write(&database, schema, value, replace).and_then(|result| {
                result.ok_or_else(|| io::Error::other("native DISP Data provider disappeared"))
            });
            return Ok(runtime_result(result.map(Value::UInt)));
        }
        let Value::Struct { fields, .. } = &value else {
            unreachable!()
        };
        let result = (|| {
            let names = data_select_sql(schema);
            let placeholders = std::iter::repeat_n("?", schema.fields.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut sql = format!(
                "INSERT INTO {} ({names}) VALUES ({placeholders})",
                data_identifier(&schema.name)
            );
            if replace {
                let primary = schema
                    .fields
                    .iter()
                    .find(|field| field.primary)
                    .expect("validated data schema has a primary field");
                let primary_name = data_identifier(&primary.name);
                let updates = schema
                    .fields
                    .iter()
                    .filter(|field| !field.primary)
                    .map(|field| {
                        let name = data_identifier(&field.name);
                        format!("{name}=excluded.{name}")
                    })
                    .collect::<Vec<_>>();
                if updates.is_empty() {
                    sql.push_str(&format!(" ON CONFLICT({primary_name}) DO NOTHING"));
                } else {
                    sql.push_str(&format!(
                        " ON CONFLICT({primary_name}) DO UPDATE SET {}",
                        updates.join(",")
                    ));
                }
            }
            let parameters = schema
                .fields
                .iter()
                .map(|field| {
                    fields
                        .get(&field.name)
                        .ok_or_else(|| io::Error::other("data value is missing a field"))
                        .and_then(|value| encode_json_value(program, value))
                })
                .collect::<io::Result<Vec<_>>>()?;
            database.execute(&sql, &parameters).map(Value::UInt)
        })();
        let _ = span;
        Ok(runtime_result(result))
    }

    fn evaluate_data_query(
        &mut self,
        program: &Program,
        schema_name: &str,
        store: &Expr,
        predicate: Option<&Expr>,
        order: Option<&crate::ast::DataOrder>,
        limit: Option<&Expr>,
    ) -> RuntimeResult<Value> {
        let Value::Database(database) = self.evaluate(program, store)? else {
            unreachable!("type checking validates the DISP Data store")
        };
        let schema = program
            .structs
            .iter()
            .find(|schema| schema.name == schema_name && schema.data)
            .expect("type checking validates the DISP Data schema");
        let limit = limit
            .map(|value| self.evaluate(program, value))
            .transpose()?;
        if let Err(error) = data_ensure_schema(&database, schema) {
            return Ok(runtime_result(Err(error)));
        }
        if database
            .is_native()
            .map_err(|error| self.error(error.to_string(), store.span))?
        {
            let mut parameters = HashMap::new();
            if let Some(predicate) = predicate {
                self.capture_data_parameters(program, schema, predicate, &mut parameters)?;
            }
            if let Some(order) = order {
                self.capture_data_parameters(program, schema, &order.key, &mut parameters)?;
            }
            let mut values = match native_data_rows(&database, schema) {
                Ok(Some(rows)) => rows,
                Ok(None) => {
                    return Ok(runtime_result(Err(io::Error::other(
                        "native DISP Data provider disappeared",
                    ))));
                }
                Err(error) => return Ok(runtime_result(Err(error))),
            };
            if let Some(predicate) = predicate {
                let mut retained = Vec::with_capacity(values.len());
                for value in values {
                    let Value::Struct { fields, .. } = &value else {
                        return Ok(runtime_result(Err(io::Error::other(
                            "stored row does not match its DISP Data schema",
                        ))));
                    };
                    match self.evaluate_data_expression(
                        program,
                        schema,
                        fields,
                        &parameters,
                        predicate,
                    )? {
                        Value::Bool(true) => retained.push(value),
                        Value::Bool(false) => {}
                        _ => unreachable!("type checking validates DISP Data predicates"),
                    }
                }
                values = retained;
            }
            if let Some(order) = order {
                for index in 1..values.len() {
                    let mut at = index;
                    while at > 0 {
                        let Value::Struct {
                            fields: left_fields,
                            ..
                        } = &values[at]
                        else {
                            unreachable!("native data tables contain schema values")
                        };
                        let Value::Struct {
                            fields: right_fields,
                            ..
                        } = &values[at - 1]
                        else {
                            unreachable!("native data tables contain schema values")
                        };
                        let left = self.evaluate_data_expression(
                            program,
                            schema,
                            left_fields,
                            &parameters,
                            &order.key,
                        )?;
                        let right = self.evaluate_data_expression(
                            program,
                            schema,
                            right_fields,
                            &parameters,
                            &order.key,
                        )?;
                        let operator = if order.descending {
                            BinaryOperator::Greater
                        } else {
                            BinaryOperator::Less
                        };
                        let ordered =
                            self.evaluate_binary(operator, left, right, order.key.span)?;
                        if ordered != Value::Bool(true) {
                            break;
                        }
                        values.swap(at, at - 1);
                        at -= 1;
                    }
                }
            }
            let amount = match limit {
                Some(Value::Int(value)) if value >= 0 => value as u64,
                Some(Value::UInt(value)) => value,
                Some(_) => {
                    return Ok(runtime_result(Err(io::Error::other(
                        "DISP Data limit is outside uint range",
                    ))));
                }
                None => DATABASE_ROW_LIMIT as u64,
            };
            if amount > DATABASE_ROW_LIMIT as u64 {
                return Ok(runtime_result(Err(io::Error::other(
                    "DISP Data limit exceeds 100000 rows",
                ))));
            }
            values.truncate(amount as usize);
            let capacity = values.len();
            return Ok(Value::Enum {
                type_name: "Result".into(),
                variant: "Ok".into(),
                payload: vec![Value::List { values, capacity }],
            });
        }
        let mut parameters = Vec::new();
        let predicate = predicate
            .map(|value| self.data_expression_sql(program, schema, value, &mut parameters))
            .transpose()?;
        let order = order
            .map(|value| {
                self.data_expression_sql(program, schema, &value.key, &mut parameters)
                    .map(|key| (key, value.descending))
            })
            .transpose()?;
        let result = (|| {
            let mut sql = format!(
                "SELECT {} FROM {}",
                data_select_sql(schema),
                data_identifier(&schema.name)
            );
            if let Some(predicate) = predicate {
                sql.push_str(" WHERE ");
                sql.push_str(&predicate);
            }
            if let Some((order, descending)) = order {
                sql.push_str(" ORDER BY ");
                sql.push_str(&order);
                sql.push_str(if descending { " DESC" } else { " ASC" });
            }
            if let Some(value) = limit {
                let amount = match value {
                    Value::Int(value) if value >= 0 => value as u64,
                    Value::UInt(value) => value,
                    _ => return Err(io::Error::other("DISP Data limit is outside uint range")),
                };
                if amount > 100_000 {
                    return Err(io::Error::other("DISP Data limit exceeds 100000 rows"));
                }
                sql.push_str(&format!(" LIMIT {amount}"));
            }
            let rows = database.query(&sql, &parameters)?;
            let values = data_decode_rows(program, schema, rows)?;
            Ok(Value::List {
                capacity: values.len(),
                values,
            })
        })();
        Ok(runtime_result(result))
    }

    fn evaluate_data_remove(
        &mut self,
        program: &Program,
        schema_name: &str,
        store: &Expr,
        predicate: &Expr,
    ) -> RuntimeResult<Value> {
        let Value::Database(database) = self.evaluate(program, store)? else {
            unreachable!("type checking validates the DISP Data store")
        };
        let schema = program
            .structs
            .iter()
            .find(|schema| schema.name == schema_name && schema.data)
            .expect("type checking validates the DISP Data schema");
        let mut parameters = Vec::new();
        if let Err(error) = data_ensure_schema(&database, schema) {
            return Ok(runtime_result(Err(error)));
        }
        if database
            .is_native()
            .map_err(|error| self.error(error.to_string(), store.span))?
        {
            let mut captured = HashMap::new();
            self.capture_data_parameters(program, schema, predicate, &mut captured)?;
            let rows = match native_data_rows(&database, schema) {
                Ok(Some(rows)) => rows,
                Ok(None) => {
                    return Ok(runtime_result(Err(io::Error::other(
                        "native DISP Data provider disappeared",
                    ))));
                }
                Err(error) => return Ok(runtime_result(Err(error))),
            };
            let mut retained = Vec::with_capacity(rows.len());
            let mut removed = 0_u64;
            for row in rows {
                let Value::Struct { fields, .. } = &row else {
                    return Ok(runtime_result(Err(io::Error::other(
                        "stored row does not match its DISP Data schema",
                    ))));
                };
                match self
                    .evaluate_data_expression(program, schema, fields, &captured, predicate)?
                {
                    Value::Bool(true) => removed += 1,
                    Value::Bool(false) => retained.push(row),
                    _ => unreachable!("type checking validates DISP Data predicates"),
                }
            }
            let result = native_data_replace_rows(&database, schema, retained)
                .and_then(|result| {
                    result.ok_or_else(|| io::Error::other("native DISP Data provider disappeared"))
                })
                .map(|()| Value::UInt(removed));
            return Ok(runtime_result(result));
        }
        let predicate = self.data_expression_sql(program, schema, predicate, &mut parameters)?;
        let result = {
            let sql = format!(
                "DELETE FROM {} WHERE {predicate}",
                data_identifier(&schema.name)
            );
            database.execute(&sql, &parameters).map(Value::UInt)
        };
        Ok(runtime_result(result))
    }

    fn evaluate(&mut self, program: &Program, expression: &Expr) -> RuntimeResult<Value> {
        match &expression.node {
            Expression::Integer(value) => {
                if *value <= i64::MAX as u128 {
                    Ok(Value::Int(*value as i64))
                } else {
                    Ok(Value::Unsigned(*value, 128))
                }
            }
            Expression::Float(value) => Ok(Value::Float(*value)),
            Expression::String(value) => Ok(Value::String(RuntimeString::literal(value.clone()))),
            Expression::Array(values) => values
                .iter()
                .map(|value| self.consume(program, value))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Expression::DataStore { path } => {
                let result = if let Some(path) = path {
                    let Value::Path(path) = self.evaluate(program, path)? else {
                        unreachable!("type checking validates data store paths")
                    };
                    let path = path.to_str().ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "data store path is not valid UTF-8",
                        )
                    });
                    path.and_then(RuntimeDatabase::open)
                } else {
                    RuntimeDatabase::native_memory()
                };
                Ok(runtime_result(result.map(Value::Database)))
            }
            Expression::DataWrite {
                value,
                store,
                replace,
            } => self.evaluate_data_write(program, value, store, *replace, expression.span),
            Expression::DataQuery {
                schema,
                store,
                predicate,
                order,
                limit,
                ..
            } => self.evaluate_data_query(
                program,
                schema,
                store,
                predicate.as_deref(),
                order.as_ref(),
                limit.as_deref(),
            ),
            Expression::DataRemove {
                schema,
                store,
                predicate,
                ..
            } => self.evaluate_data_remove(program, schema, store, predicate),
            Expression::Closure {
                move_captures,
                parameters,
                return_type,
                body,
            } => {
                let mut captures = HashMap::new();
                for (name, usage) in crate::ast::closure_capture_uses(parameters, body) {
                    let Some(place) = self.expression_place(&crate::ast::Spanned {
                        node: Expression::Identifier(name.clone()),
                        span: usage.span,
                    }) else {
                        continue;
                    };
                    let value = if *move_captures {
                        let value = self.read_place(&place).ok_or_else(|| {
                            self.error(format!("invalid capture `{name}`"), usage.span)
                        })?;
                        if !value_is_copy(program, &value) {
                            self.write_place(&place, Value::Uninitialized)
                                .ok_or_else(|| {
                                    self.error(format!("invalid capture `{name}`"), usage.span)
                                })?;
                        }
                        value
                    } else {
                        Value::CaptureReference(place, usage.mutated)
                    };
                    captures.insert(name, value);
                }
                Ok(Value::Closure(Box::new(RuntimeClosure {
                    parameters: parameters.clone(),
                    return_type: return_type.clone(),
                    body: body.clone(),
                    captures: Arc::new(StdMutex::new(captures)),
                })))
            }
            Expression::Index { object, index } => {
                let values = self.evaluate(program, object)?;
                let index_value = self.evaluate(program, index)?;
                let index = match index_value {
                    Value::Int(value) if value >= 0 => value as usize,
                    Value::UInt(value) => value as usize,
                    Value::Signed(value, _) if value >= 0 => value as usize,
                    Value::Unsigned(value, _) => usize::try_from(value)
                        .map_err(|_| self.error("array index is too large", index.span))?,
                    _ => {
                        return Err(
                            self.error("array index must be a non-negative integer", index.span)
                        );
                    }
                };
                let values = match values {
                    Value::Array(values) | Value::Slice(values) => values,
                    Value::List { values, .. } => values,
                    _ => return Err(self.error("value is not indexable", object.span)),
                };
                values.get(index).cloned().ok_or_else(|| {
                    self.error(
                        format!(
                            "array index {index} is out of bounds for length {}",
                            values.len()
                        ),
                        expression.span,
                    )
                })
            }
            Expression::Subslice { object, start, end } => {
                let values = self.evaluate(program, object)?;
                let start = self.index_value(program, start)?;
                let end = self.index_value(program, end)?;
                let values = match values {
                    Value::Array(values) | Value::Slice(values) => values,
                    Value::List { values, .. } => values,
                    Value::String(value) => {
                        if start > end
                            || end > value.text.len()
                            || !value.text.is_char_boundary(start)
                            || !value.text.is_char_boundary(end)
                        {
                            return Err(self.error(
                                "string slice is out of bounds or not on UTF-8 boundaries",
                                expression.span,
                            ));
                        }
                        return Ok(Value::String(RuntimeString::literal(
                            value.text[start..end].to_owned(),
                        )));
                    }
                    _ => return Err(self.error("value is not sliceable", object.span)),
                };
                if start > end || end > values.len() {
                    return Err(self.error(
                        format!(
                            "subslice range {start}..{end} is out of bounds for length {}",
                            values.len()
                        ),
                        expression.span,
                    ));
                }
                Ok(Value::Slice(values[start..end].to_vec()))
            }
            Expression::Character(value) => Ok(Value::Char(*value)),
            Expression::Bool(value) => Ok(Value::Bool(*value)),
            Expression::StructConstruct { name, fields, .. } => {
                let mut values = HashMap::new();
                let declaration = program
                    .structs
                    .iter()
                    .find(|declaration| declaration.name == *name);
                for field in fields {
                    let mut value = self.consume(program, &field.value)?;
                    if let Some(ty) = declaration.and_then(|declaration| {
                        declaration
                            .fields
                            .iter()
                            .find(|candidate| candidate.name == field.name)
                            .map(|field| &field.ty)
                    }) {
                        value = coerce_value(value, ty)
                            .map_err(|message| self.error(message, field.value.span))?;
                    }
                    values.insert(field.name.clone(), value);
                }
                Ok(Value::Struct {
                    type_name: name.clone(),
                    fields: values,
                })
            }
            Expression::Identifier(name) => {
                if let Some(value) = self.lookup(name) {
                    if let Value::CaptureReference(place, _) = value {
                        return self.read_place(&place).ok_or_else(|| {
                            self.error(format!("dangling capture `{name}`"), expression.span)
                        });
                    }
                    return Ok(value);
                }
                if program
                    .functions
                    .iter()
                    .any(|function| function.name == *name)
                {
                    return Ok(Value::Function(name.clone()));
                }
                match name.as_str() {
                    "None" => {
                        return Ok(Value::Enum {
                            type_name: "Option".into(),
                            variant: "None".into(),
                            payload: vec![],
                        });
                    }
                    "Some" | "Ok" | "Err" => {
                        return Ok(Value::Constructor {
                            type_name: if name == "Some" { "Option" } else { "Result" }.into(),
                            variant: name.clone(),
                        });
                    }
                    _ => {}
                }
                let candidates = find_variants(program, name);
                if candidates.len() == 1 {
                    let (owner, variant) = candidates[0];
                    if variant.payload.is_empty() {
                        return Ok(Value::Enum {
                            type_name: owner.name.clone(),
                            variant: variant.name.clone(),
                            payload: vec![],
                        });
                    }
                    return Ok(Value::Constructor {
                        type_name: owner.name.clone(),
                        variant: variant.name.clone(),
                    });
                }
                Err(self.error(format!("undefined name `{name}`"), expression.span))
            }
            Expression::FieldAccess { object, field, .. } => {
                if let Expression::Identifier(type_name) = &object.node
                    && let Some((owner, variant)) =
                        find_qualified_variant(program, type_name, field)
                {
                    if variant.payload.is_empty() {
                        return Ok(Value::Enum {
                            type_name: owner.name.clone(),
                            variant: variant.name.clone(),
                            payload: vec![],
                        });
                    }
                    return Ok(Value::Constructor {
                        type_name: owner.name.clone(),
                        variant: variant.name.clone(),
                    });
                }
                let value = match self.evaluate(program, object)? {
                    Value::Reference(place, _) => self
                        .read_place(&place)
                        .ok_or_else(|| self.error("dangling reference", object.span))?,
                    value => value,
                };
                match value {
                    Value::Struct { fields, .. } => fields.get(field).cloned().ok_or_else(|| {
                        self.error(format!("struct has no field `{field}`"), expression.span)
                    }),
                    _ => Err(self.error("field access requires a struct", object.span)),
                }
            }
            Expression::Unary { operator, operand } => {
                let value = self.evaluate(program, operand)?;
                match (operator, value) {
                    (UnaryOperator::Negate, Value::Int(value)) => {
                        value.checked_neg().map(Value::Int).ok_or_else(|| {
                            self.error("integer overflow in unary `-`", expression.span)
                        })
                    }
                    (UnaryOperator::Negate, Value::Signed(value, width)) => {
                        let negated = value.checked_neg().filter(|value| {
                            width == 128
                                || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1)
                                    .contains(value)
                        });
                        negated
                            .map(|value| Value::Signed(value, width))
                            .ok_or_else(|| {
                                self.error(
                                    format!("i{width} overflow in negation"),
                                    expression.span,
                                )
                            })
                    }
                    (UnaryOperator::Negate, Value::Unsigned(value, 128))
                        if value <= i128::MAX as u128 =>
                    {
                        Ok(Value::Signed(-(value as i128), 128))
                    }
                    (UnaryOperator::Negate, Value::Unsigned(value, 128))
                        if value == (1_u128 << 127) =>
                    {
                        Ok(Value::Signed(i128::MIN, 128))
                    }
                    (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                    (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                    _ => Err(self.error("invalid operand for unary operator", expression.span)),
                }
            }
            Expression::Binary {
                left,
                operator: BinaryOperator::And,
                right,
            } => match self.evaluate(program, left)? {
                Value::Bool(false) => Ok(Value::Bool(false)),
                Value::Bool(true) => match self.evaluate(program, right)? {
                    Value::Bool(value) => Ok(Value::Bool(value)),
                    _ => Err(self.error("right operand of `&&` is not Bool", right.span)),
                },
                _ => Err(self.error("left operand of `&&` is not Bool", left.span)),
            },
            Expression::Binary {
                left,
                operator: BinaryOperator::Or,
                right,
            } => match self.evaluate(program, left)? {
                Value::Bool(true) => Ok(Value::Bool(true)),
                Value::Bool(false) => match self.evaluate(program, right)? {
                    Value::Bool(value) => Ok(Value::Bool(value)),
                    _ => Err(self.error("right operand of `||` is not Bool", right.span)),
                },
                _ => Err(self.error("left operand of `||` is not Bool", left.span)),
            },
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate(program, left)?;
                let right_span = right.span;
                let right = self.evaluate(program, right)?;
                let (left, right) = coerce_numeric_pair(left, right)
                    .map_err(|message| self.error(message, right_span))?;
                self.evaluate_binary(*operator, left, right, expression.span)
            }
            Expression::Call { callee, arguments } => {
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "AtomicInt")
                    && field == "new"
                {
                    let Value::Int(value) = self.evaluate(program, &arguments[0])? else {
                        unreachable!("type checking validates AtomicInt.new")
                    };
                    return Ok(Value::AtomicInt(RuntimeAtomicInt(Arc::new(
                        AtomicI64::new(value),
                    ))));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Mutex")
                    && field == "new"
                {
                    return Ok(Value::Mutex(RuntimeMutex::new(
                        self.consume(program, &arguments[0])?,
                    )));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "TcpListener")
                    && field == "bind"
                {
                    let address = self.consume(program, &arguments[0])?;
                    let Value::SocketAddress(address) = address else {
                        return Err(
                            self.error("TcpListener.bind expects SocketAddress", arguments[0].span)
                        );
                    };
                    let bound = StdTcpListener::bind((address.host.as_str(), address.port))
                        .and_then(|listener| {
                            listener.set_nonblocking(true)?;
                            Ok(Value::TcpListener(RuntimeTcpListener(Arc::new(
                                StdMutex::new(Some(listener)),
                            ))))
                        });
                    return Ok(runtime_result(bound));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "UdpSocket")
                    && field == "bind"
                {
                    let address = self.consume(program, &arguments[0])?;
                    let Value::SocketAddress(address) = address else {
                        return Err(
                            self.error("UdpSocket.bind expects SocketAddress", arguments[0].span)
                        );
                    };
                    let bound =
                        StdUdpSocket::bind((address.host.as_str(), address.port)).map(|socket| {
                            Value::UdpSocket(RuntimeUdpSocket(Arc::new(StdMutex::new(Some(
                                socket,
                            )))))
                        });
                    return Ok(runtime_result(bound));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "String") {
                    return Ok(Value::String(RuntimeString::with_capacity(0)));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "SocketAddress") {
                    let host = self.evaluate(program, &arguments[0])?;
                    let host = match host {
                        Value::String(host) => host.text,
                        Value::IpAddress(address) => address.0.to_string(),
                        _ => {
                            return Err(self.error(
                                "SocketAddress host must be String, str, or IpAddress",
                                arguments[0].span,
                            ));
                        }
                    };
                    if host.is_empty() {
                        return Err(self.error("socket host cannot be empty", arguments[0].span));
                    }
                    if host.contains('\0') {
                        return Err(
                            self.error("socket host cannot contain a NUL byte", arguments[0].span)
                        );
                    }
                    let port = self.index_value(program, &arguments[1])?;
                    let port = u16::try_from(port).map_err(|_| {
                        self.error("socket port is outside 0 through 65535", arguments[1].span)
                    })?;
                    return Ok(Value::SocketAddress(RuntimeSocketAddress { host, port }));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "IpAddress")
                    && field == "parse"
                {
                    let Value::String(source) = self.evaluate(program, &arguments[0])? else {
                        unreachable!("type checking validates IP address source")
                    };
                    let parsed = source
                        .text
                        .parse::<IpAddr>()
                        .map(|address| Value::IpAddress(RuntimeIpAddress(address)))
                        .map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
                        });
                    return Ok(runtime_result(parsed));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Dns")
                    && field == "resolve"
                {
                    let Value::String(host) = self.evaluate(program, &arguments[0])? else {
                        unreachable!("type checking validates DNS host")
                    };
                    let result = resolve_ip_addresses(&host.text).map(|addresses| {
                        let values = addresses
                            .into_iter()
                            .map(|address| Value::IpAddress(RuntimeIpAddress(address)))
                            .collect::<Vec<_>>();
                        Value::List {
                            capacity: values.len(),
                            values,
                        }
                    });
                    return Ok(runtime_result(result));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Tls")
                    && matches!(field.as_str(), "connect" | "connect_timeout")
                {
                    let Value::TcpStream(stream) = self.consume(program, &arguments[0])? else {
                        unreachable!("type checking validates TLS source stream")
                    };
                    let Value::String(server_name) = self.evaluate(program, &arguments[1])? else {
                        unreachable!("type checking validates TLS server name")
                    };
                    let timeout = if field == "connect_timeout" {
                        let Value::Duration(duration) = self.evaluate(program, &arguments[2])?
                        else {
                            unreachable!("type checking validates TLS handshake timeout")
                        };
                        Some(duration)
                    } else {
                        None
                    };
                    return Ok(Value::Future(RuntimeFuture::operation(
                        FutureWork::TlsConnect(stream, server_name.text, timeout),
                    )));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Http")
                    && matches!(
                        field.as_str(),
                        "get"
                            | "get_timeout"
                            | "post"
                            | "post_timeout"
                            | "post_json"
                            | "post_json_timeout"
                            | "put"
                            | "put_timeout"
                            | "patch"
                            | "patch_timeout"
                            | "delete"
                            | "delete_timeout"
                            | "request"
                    )
                {
                    let (method, url_index) = match field.as_str() {
                        "request" => {
                            let Value::String(method) = self.evaluate(program, &arguments[0])?
                            else {
                                unreachable!("type checking validates HTTP method")
                            };
                            (method.text, 1)
                        }
                        name if name.starts_with("post") => ("POST".into(), 0),
                        name if name.starts_with("put") => ("PUT".into(), 0),
                        name if name.starts_with("patch") => ("PATCH".into(), 0),
                        name if name.starts_with("delete") => ("DELETE".into(), 0),
                        _ => ("GET".into(), 0),
                    };
                    let url = match self.evaluate(program, &arguments[url_index])? {
                        Value::String(url) => url.text,
                        Value::Url(url) => url.text,
                        _ => unreachable!("type checking validates HTTP URL"),
                    };
                    if field == "request" {
                        let request = http_method(&method)
                            .and_then(|method| {
                                parse_http_url(&url).map(|url| RuntimeHttpRequest {
                                    method,
                                    url: url.to_string(),
                                    headers: vec![],
                                    body: vec![],
                                })
                            })
                            .map(Value::HttpRequest);
                        return Ok(runtime_result(request));
                    }
                    let (body, headers) = if matches!(method.as_str(), "POST" | "PUT" | "PATCH") {
                        let value = self.evaluate(program, &arguments[1])?;
                        let text = matches!(value, Value::String(_));
                        let json = matches!(value, Value::Json(_));
                        let body = runtime_http_body(value)
                            .unwrap_or_else(|| unreachable!("type checking validates HTTP body"));
                        let headers = if json {
                            vec![("Content-Type".into(), "application/json".into())]
                        } else if text {
                            vec![("Content-Type".into(), "text/plain; charset=utf-8".into())]
                        } else {
                            vec![]
                        };
                        (body, headers)
                    } else {
                        (vec![], vec![])
                    };
                    let timeout = if field.ends_with("_timeout") {
                        let index = if body.is_empty()
                            && !matches!(method.as_str(), "POST" | "PUT" | "PATCH")
                        {
                            1
                        } else {
                            2
                        };
                        let Value::Duration(duration) =
                            self.evaluate(program, &arguments[index])?
                        else {
                            unreachable!("type checking validates HTTP request timeout")
                        };
                        duration
                    } else {
                        StdDuration::from_secs(30)
                    };
                    return Ok(Value::Future(RuntimeFuture::operation(
                        FutureWork::HttpRequest(
                            RuntimeHttpRequest {
                                method,
                                url,
                                headers,
                                body,
                            },
                            timeout,
                        ),
                    )));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Path") {
                    let value = self.evaluate(program, &arguments[0])?;
                    let path = runtime_path(value).ok_or_else(|| {
                        self.error("Path source must be String or str", arguments[0].span)
                    })?;
                    if path.as_os_str().to_string_lossy().contains('\0') {
                        return Err(self.error("Path cannot contain a NUL byte", arguments[0].span));
                    }
                    return Ok(Value::Path(path));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Url") {
                    let Value::String(source) = self.evaluate(program, &arguments[0])? else {
                        unreachable!("type checking validates URL source")
                    };
                    return Ok(runtime_result(
                        parse_http_url(&source.text)
                            .map(|_| Value::Url(RuntimeUrl { text: source.text })),
                    ));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Json") {
                    let Value::String(source) = self.evaluate(program, &arguments[0])? else {
                        unreachable!("type checking validates JSON source")
                    };
                    return Ok(runtime_result(runtime_json(source.text).map(Value::Json)));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Json")
                {
                    let make = |text: String| runtime_json(text).map(Value::Json);
                    return match field.as_str() {
                        "null" => Ok(Value::Json(runtime_json("null".into()).unwrap())),
                        "bool" => {
                            let Value::Bool(value) = self.evaluate(program, &arguments[0])? else {
                                unreachable!("type checking validates Json.bool")
                            };
                            Ok(Value::Json(
                                runtime_json(if value { "true" } else { "false" }.into()).unwrap(),
                            ))
                        }
                        "int" => {
                            let value = self.evaluate(program, &arguments[0])?;
                            let text = match value {
                                Value::Int(value) => value.to_string(),
                                Value::Signed(value, _) => value.to_string(),
                                _ => unreachable!("type checking validates Json.int"),
                            };
                            Ok(Value::Json(runtime_json(text).unwrap()))
                        }
                        "uint" => {
                            let value = self.evaluate(program, &arguments[0])?;
                            let text = match value {
                                Value::UInt(value) => value.to_string(),
                                Value::Unsigned(value, _) => value.to_string(),
                                _ => unreachable!("type checking validates Json.uint"),
                            };
                            Ok(Value::Json(runtime_json(text).unwrap()))
                        }
                        "float" => {
                            let value = self.evaluate(program, &arguments[0])?;
                            let value = match value {
                                Value::Float(value) => value,
                                Value::Float32(value) => f64::from(value),
                                _ => unreachable!("type checking validates Json.float"),
                            };
                            let result = if value.is_finite() {
                                make(value.to_string())
                            } else {
                                Err(json_conversion_error(
                                    "JSON cannot represent NaN or infinity",
                                ))
                            };
                            Ok(runtime_result(result))
                        }
                        "string" => {
                            let Value::String(value) = self.evaluate(program, &arguments[0])?
                            else {
                                unreachable!("type checking validates Json.string")
                            };
                            Ok(runtime_result(
                                json_escape_string(&value.text).and_then(make),
                            ))
                        }
                        "array" => {
                            let Value::List { values, .. } =
                                self.evaluate(program, &arguments[0])?
                            else {
                                unreachable!("type checking validates Json.array")
                            };
                            let mut text = String::from("[");
                            for (index, value) in values.into_iter().enumerate() {
                                let Value::Json(value) = value else {
                                    unreachable!("type checking validates Json.array elements")
                                };
                                let additional = usize::from(index != 0)
                                    .checked_add(value.text.len())
                                    .and_then(|amount| amount.checked_add(1));
                                if additional
                                    .and_then(|amount| text.len().checked_add(amount))
                                    .is_none_or(|length| length > HTTP_BODY_LIMIT)
                                {
                                    return Ok(runtime_result(Err(json_conversion_error(
                                        "JSON document exceeds the 16 MiB limit",
                                    ))));
                                }
                                if index != 0 {
                                    text.push(',');
                                }
                                text.push_str(&value.text);
                            }
                            text.push(']');
                            Ok(runtime_result(make(text)))
                        }
                        "object" => {
                            let Value::Map { entries, .. } =
                                self.evaluate(program, &arguments[0])?
                            else {
                                unreachable!("type checking validates Json.object")
                            };
                            let mut text = String::from("{");
                            for (index, (key, value)) in entries.into_iter().enumerate() {
                                let Value::String(key) = key else {
                                    unreachable!("type checking validates Json.object keys")
                                };
                                let Value::Json(value) = value else {
                                    unreachable!("type checking validates Json.object values")
                                };
                                let escaped = match json_escape_string(&key.text) {
                                    Ok(escaped) => escaped,
                                    Err(error) => return Ok(runtime_result(Err(error))),
                                };
                                let additional = usize::from(index != 0)
                                    .checked_add(escaped.len())
                                    .and_then(|amount| amount.checked_add(1))
                                    .and_then(|amount| amount.checked_add(value.text.len()))
                                    .and_then(|amount| amount.checked_add(1));
                                if additional
                                    .and_then(|amount| text.len().checked_add(amount))
                                    .is_none_or(|length| length > HTTP_BODY_LIMIT)
                                {
                                    return Ok(runtime_result(Err(json_conversion_error(
                                        "JSON document exceeds the 16 MiB limit",
                                    ))));
                                }
                                if index != 0 {
                                    text.push(',');
                                }
                                text.push_str(&escaped);
                                text.push(':');
                                text.push_str(&value.text);
                            }
                            text.push('}');
                            Ok(runtime_result(make(text)))
                        }
                        "from" => {
                            let value = self.evaluate(program, &arguments[0])?;
                            Ok(runtime_result(
                                encode_json_value(program, &value).map(Value::Json),
                            ))
                        }
                        _ => Err(self.error("unknown Json constructor", expression.span)),
                    };
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && field == "from_json"
                    && let Expression::Identifier(owner) = &object.node
                    && (program
                        .structs
                        .iter()
                        .any(|declaration| declaration.name == *owner)
                        || program
                            .enums
                            .iter()
                            .any(|declaration| declaration.name == *owner))
                {
                    let Value::Json(json) = self.evaluate(program, &arguments[0])? else {
                        unreachable!("type checking validates nominal JSON source")
                    };
                    let target = TypeName {
                        name: owner.clone(),
                        arguments: vec![],
                        qualifier: TypeQualifier::Owned,
                        span: object.span,
                    };
                    return Ok(runtime_result(decode_json_value(
                        program,
                        &target,
                        &HashMap::new(),
                        &json,
                    )));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "String")
                {
                    return match field.as_str() {
                        "new" => Ok(Value::String(RuntimeString::with_capacity(0))),
                        "with_capacity" => {
                            let value = self.evaluate(program, &arguments[0])?;
                            let capacity = match value {
                                Value::Int(x) if x >= 0 => x as usize,
                                Value::UInt(x) => x as usize,
                                _ => {
                                    return Err(self.error(
                                        "capacity must be a non-negative integer",
                                        arguments[0].span,
                                    ));
                                }
                            };
                            Ok(Value::String(RuntimeString::with_capacity(capacity)))
                        }
                        _ => Err(self.error("unknown String constructor", expression.span)),
                    };
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "CString")
                    && field == "new"
                {
                    let source = self.evaluate(program, &arguments[0])?;
                    let bytes = match source {
                        Value::String(value) => value.text.into_bytes(),
                        Value::CStr(value) | Value::CString(value) => value.text().into_bytes(),
                        _ => unreachable!("type checking validates CString source"),
                    };
                    return Ok(match RuntimeCString::new(&bytes) {
                        Ok(value) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Ok".into(),
                            payload: vec![Value::CString(value)],
                        },
                        Err(message) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Err".into(),
                            payload: vec![Value::String(RuntimeString::literal(message.into()))],
                        },
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Memory")
                    && field == "allocate"
                {
                    let size = self.index_value(program, &arguments[0])?;
                    let alignment = self.index_value(program, &arguments[1])?;
                    return Ok(match RuntimeMemory::new(size, alignment) {
                        Ok(memory) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Ok".into(),
                            payload: vec![Value::Memory(memory)],
                        },
                        Err(message) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Err".into(),
                            payload: vec![Value::String(RuntimeString::literal(message.into()))],
                        },
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "List")
                {
                    if field == "of" {
                        let mut values = Vec::with_capacity(arguments.len());
                        for argument in arguments {
                            values.push(self.consume(program, argument)?);
                        }
                        return Ok(Value::List {
                            capacity: values.len(),
                            values,
                        });
                    }
                    let capacity = if field == "with_capacity" {
                        self.index_value(program, &arguments[0])?
                    } else {
                        0
                    };
                    return Ok(Value::List {
                        values: Vec::new(),
                        capacity,
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Map")
                {
                    if field == "of" {
                        let mut entries = Vec::with_capacity(arguments.len() / 2);
                        for pair in arguments.chunks_exact(2) {
                            let key = self.consume(program, &pair[0])?;
                            let value = self.consume(program, &pair[1])?;
                            if let Some(entry) =
                                entries.iter_mut().find(|(candidate, _)| candidate == &key)
                            {
                                entry.1 = value;
                            } else {
                                entries.push((key, value));
                            }
                        }
                        return Ok(Value::Map {
                            capacity: entries.len(),
                            entries,
                        });
                    }
                    let capacity = if field == "with_capacity" {
                        self.index_value(program, &arguments[0])?
                    } else {
                        0
                    };
                    return Ok(Value::Map {
                        entries: Vec::new(),
                        capacity,
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Set")
                {
                    if field == "of" {
                        let mut values = Vec::with_capacity(arguments.len());
                        for argument in arguments {
                            let value = self.consume(program, argument)?;
                            if !values.contains(&value) {
                                values.push(value);
                            }
                        }
                        return Ok(Value::Set {
                            capacity: values.len(),
                            values,
                        });
                    }
                    let capacity = if field == "with_capacity" {
                        self.index_value(program, &arguments[0])?
                    } else {
                        0
                    };
                    return Ok(Value::Set {
                        values: Vec::new(),
                        capacity,
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Async")
                {
                    return match field.as_str() {
                        "yield" => Ok(Value::Future(RuntimeFuture::yielding())),
                        "spawn" => {
                            let value = self.consume(program, &arguments[0])?;
                            let Value::Future(future) = value else {
                                return Err(
                                    self.error("Async.spawn requires a Future", arguments[0].span)
                                );
                            };
                            let task = RuntimeTask::new(future);
                            self.tasks.push(Arc::downgrade(&task.0));
                            Ok(Value::Task(task))
                        }
                        "sleep" => {
                            let Value::Duration(duration) =
                                self.evaluate(program, &arguments[0])?
                            else {
                                return Err(
                                    self.error("Async.sleep expects Duration", arguments[0].span)
                                );
                            };
                            Ok(Value::Future(RuntimeFuture::operation(FutureWork::Sleep(
                                duration,
                            ))))
                        }
                        "connect" | "connect_timeout" => {
                            let value = self.consume(program, &arguments[0])?;
                            let Value::SocketAddress(address) = value else {
                                return Err(self.error(
                                    "TCP connect expects SocketAddress",
                                    arguments[0].span,
                                ));
                            };
                            let timeout = if field == "connect_timeout" {
                                let Value::Duration(duration) =
                                    self.evaluate(program, &arguments[1])?
                                else {
                                    unreachable!("type checking validates connect timeout")
                                };
                                Some(duration)
                            } else {
                                None
                            };
                            Ok(Value::Future(RuntimeFuture::operation(
                                FutureWork::Connect(address, timeout),
                            )))
                        }
                        "resolve" | "resolve_timeout" => {
                            let Value::String(host) = self.evaluate(program, &arguments[0])? else {
                                unreachable!("type checking validates DNS host")
                            };
                            let timeout = if field == "resolve_timeout" {
                                let Value::Duration(duration) =
                                    self.evaluate(program, &arguments[1])?
                                else {
                                    unreachable!("type checking validates DNS timeout")
                                };
                                Some(duration)
                            } else {
                                None
                            };
                            Ok(Value::Future(RuntimeFuture::operation(
                                FutureWork::Resolve(host.text, timeout),
                            )))
                        }
                        "read_text" | "read_bytes" => {
                            let value = self.consume(program, &arguments[0])?;
                            let Value::Path(path) = value else {
                                return Err(self.error(
                                    "async file operation expects Path",
                                    arguments[0].span,
                                ));
                            };
                            let work = if field == "read_text" {
                                FutureWork::ReadText(path)
                            } else {
                                FutureWork::ReadBytes(path)
                            };
                            Ok(Value::Future(RuntimeFuture::operation(work)))
                        }
                        "write_text" | "write_bytes" => {
                            let path = self.consume(program, &arguments[0])?;
                            let Value::Path(path) = path else {
                                return Err(self.error(
                                    "async file operation expects Path",
                                    arguments[0].span,
                                ));
                            };
                            let value = self.consume(program, &arguments[1])?;
                            let work = if field == "write_text" {
                                let Value::String(text) = value else {
                                    return Err(self.error(
                                        "Async.write_text expects owned String",
                                        arguments[1].span,
                                    ));
                                };
                                FutureWork::WriteText(path, text)
                            } else {
                                let Value::List { values, .. } = value else {
                                    return Err(self.error(
                                        "Async.write_bytes expects owned List<u8>",
                                        arguments[1].span,
                                    ));
                                };
                                let mut bytes = Vec::with_capacity(values.len());
                                for value in values {
                                    let Value::Unsigned(value, 8) = value else {
                                        return Err(self.error(
                                            "Async.write_bytes expects owned List<u8>",
                                            arguments[1].span,
                                        ));
                                    };
                                    bytes.push(value as u8);
                                }
                                FutureWork::WriteBytes(path, bytes)
                            };
                            Ok(Value::Future(RuntimeFuture::operation(work)))
                        }
                        _ => Err(self.error("unknown Async operation", callee.span)),
                    };
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(owner) = &object.node
                    && matches!(
                        owner.as_str(),
                        "Path"
                            | "File"
                            | "Directory"
                            | "Time"
                            | "Duration"
                            | "Environment"
                            | "Process"
                            | "Database"
                            | "DataStore"
                    )
                {
                    if owner == "Environment" {
                        return match field.as_str() {
                            "arguments" => {
                                let values = self
                                    .program_arguments
                                    .iter()
                                    .cloned()
                                    .map(|value| Value::String(RuntimeString::literal(value)))
                                    .collect::<Vec<_>>();
                                Ok(Value::List {
                                    capacity: values.len(),
                                    values,
                                })
                            }
                            "get" => {
                                let Value::String(name) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates environment names")
                                };
                                if name.text.is_empty()
                                    || name.text.contains('=')
                                    || name.text.contains('\0')
                                {
                                    return Err(self.error(
                                        "environment variable name cannot be empty or contain '=' or NUL",
                                        arguments[0].span,
                                    ));
                                }
                                let value = match std::env::var_os(&name.text) {
                                    Some(value) => Some(value.into_string().map_err(|_| {
                                        self.error(
                                            "environment variable value is not valid UTF-8",
                                            arguments[0].span,
                                        )
                                    })?),
                                    None => None,
                                };
                                Ok(option_value(
                                    value.map(|value| Value::String(RuntimeString::literal(value))),
                                ))
                            }
                            _ => Err(self.error("unknown Environment operation", expression.span)),
                        };
                    }
                    if owner == "Process" {
                        let path = if field == "command" {
                            self.consume(program, &arguments[0])?
                        } else {
                            self.evaluate(program, &arguments[0])?
                        };
                        let Value::Path(program_path) = path else {
                            unreachable!("type checking validates process paths")
                        };
                        if field == "command" {
                            return Ok(Value::ProcessCommand(Box::new(RuntimeProcessCommand {
                                program: program_path,
                                arguments: Vec::new(),
                                directory: None,
                                environment: Vec::new(),
                                clear_environment: false,
                                input: Vec::new(),
                                timeout: None,
                            })));
                        }
                        let Value::List { values, .. } = self.evaluate(program, &arguments[1])?
                        else {
                            unreachable!("type checking validates process arguments")
                        };
                        let mut process_arguments = Vec::with_capacity(values.len());
                        for value in values {
                            let Value::String(value) = value else {
                                unreachable!("type checking validates process arguments")
                            };
                            process_arguments.push(value.text);
                        }
                        return Ok(runtime_result(execute_process(RuntimeProcessCommand {
                            program: program_path,
                            arguments: process_arguments,
                            directory: None,
                            environment: Vec::new(),
                            clear_environment: false,
                            input: Vec::new(),
                            timeout: None,
                        })));
                    }
                    if owner == "Database" {
                        let result = match field.as_str() {
                            "memory" => RuntimeDatabase::open(":memory:"),
                            "open" => {
                                let Value::Path(path) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates database paths")
                                };
                                let path = path.to_str().ok_or_else(|| {
                                    io::Error::new(
                                        io::ErrorKind::InvalidInput,
                                        "database path is not valid UTF-8",
                                    )
                                });
                                path.and_then(RuntimeDatabase::open)
                            }
                            _ => unreachable!("type checking validates Database constructors"),
                        };
                        return Ok(runtime_result(result.map(Value::Database)));
                    }
                    if owner == "Path" {
                        let value = self.evaluate(program, &arguments[0])?;
                        let path = runtime_path(value).ok_or_else(|| {
                            self.error("Path source must be String or str", arguments[0].span)
                        })?;
                        if path.as_os_str().to_string_lossy().contains('\0') {
                            return Err(
                                self.error("Path cannot contain a NUL byte", arguments[0].span)
                            );
                        }
                        return Ok(Value::Path(path));
                    }
                    if owner == "Time" {
                        return match field.as_str() {
                            "now" => Ok(Value::Instant(StdInstant::now())),
                            "unix_seconds" => Ok(Value::UInt(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                            )),
                            "sleep" => {
                                let Value::Duration(duration) =
                                    self.evaluate(program, &arguments[0])?
                                else {
                                    return Err(
                                        self.error("sleep expects Duration", arguments[0].span)
                                    );
                                };
                                thread::sleep(duration);
                                Ok(Value::Unit)
                            }
                            _ => Err(self.error("unknown Time operation", expression.span)),
                        };
                    }
                    if owner == "Duration" {
                        let amount = self.index_value(program, &arguments[0])? as u64;
                        let factor = match field.as_str() {
                            "from_nanos" => 1,
                            "from_millis" => 1_000_000,
                            _ => 1_000_000_000,
                        };
                        let nanos = amount
                            .checked_mul(factor)
                            .ok_or_else(|| self.error("Duration overflow", expression.span))?;
                        return Ok(Value::Duration(StdDuration::from_nanos(nanos)));
                    }
                    let path =
                        runtime_path(self.evaluate(program, &arguments[0])?).ok_or_else(|| {
                            self.error("filesystem path must be Path", arguments[0].span)
                        })?;
                    if owner == "File" {
                        return match field.as_str() {
                            "read_text" => Ok(runtime_result(
                                fs::read_to_string(path)
                                    .map(|text| Value::String(RuntimeString::literal(text))),
                            )),
                            "read_bytes" => Ok(runtime_result(fs::read(path).map(|bytes| {
                                let values = bytes
                                    .into_iter()
                                    .map(|value| Value::Unsigned(value as u128, 8))
                                    .collect::<Vec<_>>();
                                Value::List {
                                    capacity: values.len(),
                                    values,
                                }
                            }))),
                            "size" => Ok(runtime_result(
                                fs::metadata(path).map(|metadata| Value::UInt(metadata.len())),
                            )),
                            "modified_seconds" => Ok(runtime_result(
                                fs::metadata(path)
                                    .and_then(|metadata| metadata.modified())
                                    .and_then(|modified| {
                                        modified
                                            .duration_since(UNIX_EPOCH)
                                            .map_err(std::io::Error::other)
                                    })
                                    .map(|duration| Value::UInt(duration.as_secs())),
                            )),
                            "write_text" | "append_text" => {
                                let Value::String(text) = self.evaluate(program, &arguments[1])?
                                else {
                                    return Err(self.error(
                                        "file text must be String or str",
                                        arguments[1].span,
                                    ));
                                };
                                let result = if field == "append_text" {
                                    use std::io::Write;
                                    fs::OpenOptions::new()
                                        .create(true)
                                        .append(true)
                                        .open(path)
                                        .and_then(|mut file| file.write_all(text.text.as_bytes()))
                                } else {
                                    fs::write(path, text.text)
                                };
                                Ok(runtime_result(result.map(|_| Value::Unit)))
                            }
                            "write_bytes" | "append_bytes" => {
                                let values = self.evaluate(program, &arguments[1])?;
                                let values = match values {
                                    Value::List { values, .. } | Value::Slice(values) => values,
                                    _ => {
                                        return Err(self.error(
                                            "file bytes must be List<u8> or a u8 slice",
                                            arguments[1].span,
                                        ));
                                    }
                                };
                                let bytes =
                                    values
                                        .into_iter()
                                        .map(|value| match value {
                                            Value::Unsigned(value, 8) => Ok(value as u8),
                                            _ => Err(self
                                                .error("file byte is not u8", arguments[1].span)),
                                        })
                                        .collect::<Result<Vec<_>, _>>()?;
                                let result = if field == "append_bytes" {
                                    use std::io::Write;
                                    fs::OpenOptions::new()
                                        .create(true)
                                        .append(true)
                                        .open(path)
                                        .and_then(|mut file| file.write_all(&bytes))
                                } else {
                                    fs::write(path, bytes)
                                };
                                Ok(runtime_result(result.map(|_| Value::Unit)))
                            }
                            "exists" => Ok(Value::Bool(path.is_file())),
                            "remove" => {
                                Ok(runtime_result(fs::remove_file(path).map(|_| Value::Unit)))
                            }
                            "copy" => {
                                let to = runtime_path(self.evaluate(program, &arguments[1])?)
                                    .ok_or_else(|| {
                                        self.error(
                                            "filesystem path must be Path",
                                            arguments[1].span,
                                        )
                                    })?;
                                Ok(runtime_result(fs::copy(path, to).map(|_| Value::Unit)))
                            }
                            "move" => {
                                let to = runtime_path(self.evaluate(program, &arguments[1])?)
                                    .ok_or_else(|| {
                                        self.error(
                                            "filesystem path must be Path",
                                            arguments[1].span,
                                        )
                                    })?;
                                Ok(runtime_result(fs::rename(path, to).map(|_| Value::Unit)))
                            }
                            _ => Err(self.error("unknown File operation", expression.span)),
                        };
                    }
                    return match field.as_str() {
                        "exists" => Ok(Value::Bool(path.is_dir())),
                        "create" => Ok(runtime_result(fs::create_dir(path).map(|_| Value::Unit))),
                        "create_all" => Ok(runtime_result(
                            fs::create_dir_all(path).map(|_| Value::Unit),
                        )),
                        "remove" => Ok(runtime_result(fs::remove_dir(path).map(|_| Value::Unit))),
                        "read" => Ok(runtime_result(
                            fs::read_dir(path)
                                .and_then(|entries| {
                                    entries
                                        .map(|entry| entry.map(|entry| Value::Path(entry.path())))
                                        .collect::<Result<Vec<_>, _>>()
                                })
                                .map(|values| Value::List {
                                    capacity: values.len(),
                                    values,
                                }),
                        )),
                        _ => Err(self.error("unknown Directory operation", expression.span)),
                    };
                }
                if let Expression::Identifier(name) = &callee.node
                    && is_numeric_name(name)
                {
                    let value = self.evaluate(program, &arguments[0])?;
                    return coerce_value(
                        value,
                        &crate::ast::TypeName {
                            name: name.clone(),
                            arguments: vec![],
                            qualifier: crate::ast::TypeQualifier::Owned,
                            span: callee.span,
                        },
                    )
                    .map_err(|message| self.error(message, expression.span));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(name) = &object.node
                    && is_numeric_name(name)
                    && field == "try_from"
                {
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Map {
                            mut entries,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let field = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains_key" => "has",
                            "insert" => "set",
                            other => other,
                        };
                        match field {
                            "len" => return Ok(Value::UInt(entries.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(entries.is_empty())),
                            "has" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(
                                    entries.iter().any(|(candidate, _)| candidate == &key),
                                ));
                            }
                            "get" | "get_mut" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(
                                    match entries
                                        .iter()
                                        .position(|(candidate, _)| candidate == &key)
                                    {
                                        Some(index) => {
                                            let mut value = place.clone();
                                            value.fields.push(PlaceSegment::MapValue(index));
                                            Value::Enum {
                                                type_name: "Option".into(),
                                                variant: "Some".into(),
                                                payload: vec![Value::Reference(
                                                    value,
                                                    field == "get_mut",
                                                )],
                                            }
                                        }
                                        None => Value::Enum {
                                            type_name: "Option".into(),
                                            variant: "None".into(),
                                            payload: vec![],
                                        },
                                    },
                                );
                            }
                            "set" => {
                                let key = self.consume(program, &arguments[0])?;
                                let value = self.consume(program, &arguments[1])?;
                                let old = if let Some(entry) =
                                    entries.iter_mut().find(|(candidate, _)| candidate == &key)
                                {
                                    Some(std::mem::replace(&mut entry.1, value))
                                } else {
                                    grow_list_capacity(&mut capacity, entries.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    entries.push((key, value));
                                    None
                                };
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(match old {
                                    Some(value) => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "Some".into(),
                                        payload: vec![value],
                                    },
                                    None => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    },
                                });
                            }
                            "remove" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                let removed = entries
                                    .iter()
                                    .position(|(candidate, _)| candidate == &key)
                                    .map(|index| entries.remove(index).1);
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(match removed {
                                    Some(value) => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "Some".into(),
                                        payload: vec![value],
                                    },
                                    None => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    },
                                });
                            }
                            "clear" => entries.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Map { entries, capacity })
                            .ok_or_else(|| {
                                self.error("Map mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Set {
                            mut values,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let field = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains" => "has",
                            "insert" => "add",
                            other => other,
                        };
                        match field {
                            "len" => return Ok(Value::UInt(values.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(values.is_empty())),
                            "has" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(values.contains(&value)));
                            }
                            "add" => {
                                let value = self.consume(program, &arguments[0])?;
                                let added = if values.contains(&value) {
                                    false
                                } else {
                                    grow_list_capacity(&mut capacity, values.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    values.push(value);
                                    true
                                };
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(added));
                            }
                            "remove" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let removed = values
                                    .iter()
                                    .position(|candidate| candidate == &value)
                                    .map(|index| values.remove(index))
                                    .is_some();
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(removed));
                            }
                            "clear" => values.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Set { values, capacity })
                            .ok_or_else(|| {
                                self.error("Set mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    let value = self.evaluate(program, &arguments[0])?;
                    return Ok(
                        match coerce_value(
                            value,
                            &crate::ast::TypeName {
                                name: name.clone(),
                                arguments: vec![],
                                qualifier: crate::ast::TypeQualifier::Owned,
                                span: object.span,
                            },
                        ) {
                            Ok(value) => Value::Enum {
                                type_name: "Result".into(),
                                variant: "Ok".into(),
                                payload: vec![value],
                            },
                            Err(message) => Value::Enum {
                                type_name: "Result".into(),
                                variant: "Err".into(),
                                payload: vec![Value::String(RuntimeString::literal(message))],
                            },
                        },
                    );
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(
                        field.as_str(),
                        "wrapping_add"
                            | "wrapping_sub"
                            | "wrapping_mul"
                            | "saturating_add"
                            | "saturating_sub"
                            | "saturating_mul"
                    )
                {
                    let left = self.evaluate(program, object)?;
                    let right = coerce_like(self.evaluate(program, &arguments[0])?, &left)
                        .map_err(|message| self.error(message, arguments[0].span))?;
                    return integer_method(field, left, right)
                        .map_err(|message| self.error(message, expression.span));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && !matches!(&object.node, Expression::Identifier(name) if program.enums.iter().any(|declaration| declaration.name == *name))
                {
                    if field == "lock" && arguments.is_empty() {
                        let Value::Mutex(mutex) = self.evaluate(program, object)? else {
                            return Err(self.error("Mutex method requires a Mutex", object.span));
                        };
                        return Ok(Value::MutexGuard(mutex.lock()));
                    }
                    if field == "share" && arguments.is_empty() {
                        return match self.evaluate(program, object)? {
                            Value::Mutex(mutex) => Ok(Value::Mutex(mutex)),
                            Value::AtomicInt(atomic) => Ok(Value::AtomicInt(atomic)),
                            _ => {
                                Err(self.error("share requires a Mutex or AtomicInt", object.span))
                            }
                        };
                    }
                    if matches!(
                        field.as_str(),
                        "share" | "load" | "store" | "add" | "fetch_add"
                    ) && let Value::AtomicInt(atomic) = self.evaluate(program, object)?
                    {
                        return match field.as_str() {
                            "share" => Ok(Value::AtomicInt(atomic)),
                            "load" => Ok(Value::Int(atomic.0.load(Ordering::SeqCst))),
                            "store" => {
                                let Value::Int(value) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates AtomicInt.store")
                                };
                                atomic.0.store(value, Ordering::SeqCst);
                                Ok(Value::Unit)
                            }
                            "add" | "fetch_add" => {
                                let Value::Int(value) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates AtomicInt.add")
                                };
                                let (previous, next) = atomic
                                    .add(value, expression.span)
                                    .map_err(RuntimeFault::Error)?;
                                Ok(Value::Int(if field == "add" { next } else { previous }))
                            }
                            _ => unreachable!(),
                        };
                    }
                    if matches!(field.as_str(), "offset" | "read" | "write")
                        && let Value::Reference(mut place, mutable) =
                            self.evaluate(program, object)?
                    {
                        return match field.as_str() {
                            "offset" => {
                                let offset = match self.evaluate(program, &arguments[0])? {
                                    Value::Int(value) => value,
                                    Value::Signed(value, _) => {
                                        i64::try_from(value).map_err(|_| {
                                            self.error(
                                                "raw pointer offset is out of range",
                                                arguments[0].span,
                                            )
                                        })?
                                    }
                                    _ => unreachable!("type checking validates pointer offset"),
                                };
                                let Some(PlaceSegment::Index(index)) = place.fields.last_mut()
                                else {
                                    return Err(self.error(
                                        "interpreter raw pointer has no addressable element",
                                        object.span,
                                    ));
                                };
                                *index =
                                    index.checked_add_signed(offset as isize).ok_or_else(|| {
                                        self.error("raw pointer offset overflow", arguments[0].span)
                                    })?;
                                Ok(Value::Reference(place, mutable))
                            }
                            "read" => self.read_place(&place).ok_or_else(|| {
                                self.error("raw pointer read is out of bounds", expression.span)
                            }),
                            "write" if mutable => {
                                let value = self.evaluate(program, &arguments[0])?;
                                self.write_place(&place, value).ok_or_else(|| {
                                    self.error(
                                        "raw pointer write is out of bounds",
                                        expression.span,
                                    )
                                })?;
                                Ok(Value::Unit)
                            }
                            "write" => Err(self.error(
                                "cannot write through a const raw pointer",
                                expression.span,
                            )),
                            _ => unreachable!(),
                        };
                    }
                    if field == "join" && arguments.is_empty() {
                        let Value::Thread(handle) = self.consume(program, object)? else {
                            return Err(
                                self.error("Thread.join requires a Thread value", object.span)
                            );
                        };
                        return handle.join(expression.span).map_err(RuntimeFault::Error);
                    }
                    if let Value::TcpListener(listener) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "accept" | "accept_timeout" => {
                                let timeout = if field == "accept_timeout" {
                                    let Value::Duration(duration) =
                                        self.evaluate(program, &arguments[0])?
                                    else {
                                        unreachable!("type checking validates accept timeout")
                                    };
                                    Some(duration)
                                } else {
                                    None
                                };
                                Ok(Value::Future(RuntimeFuture::operation(FutureWork::Accept(
                                    listener, timeout,
                                ))))
                            }
                            "local_port" => {
                                let guard = listener.0.lock().map_err(|_| {
                                    self.error("TCP listener state is poisoned", object.span)
                                })?;
                                let port = match guard.as_ref() {
                                    Some(listener) => listener
                                        .local_addr()
                                        .map(|address| Value::UInt(address.port() as u64)),
                                    None => Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP listener is closed",
                                    )),
                                };
                                Ok(runtime_result(port))
                            }
                            "close" => {
                                let mut guard = listener.0.lock().map_err(|_| {
                                    self.error("TCP listener state is poisoned", object.span)
                                })?;
                                guard.take();
                                Ok(Value::Unit)
                            }
                            _ => Err(self.error("unknown TcpListener operation", expression.span)),
                        };
                    }
                    if let Value::TcpStream(stream) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "read" | "read_async" | "read_async_timeout" => {
                                let limit = self.index_value(program, &arguments[0])?;
                                if limit > 16 * 1024 * 1024 {
                                    return Err(self.error(
                                        "TCP read limit exceeds the 16 MiB safety limit",
                                        arguments[0].span,
                                    ));
                                }
                                if field != "read" {
                                    let timeout = if field == "read_async_timeout" {
                                        let Value::Duration(duration) =
                                            self.evaluate(program, &arguments[1])?
                                        else {
                                            unreachable!("type checking validates read timeout")
                                        };
                                        Some(duration)
                                    } else {
                                        None
                                    };
                                    return Ok(Value::Future(RuntimeFuture::operation(
                                        FutureWork::SocketRead(stream, limit, timeout),
                                    )));
                                }
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TCP stream state is poisoned", object.span)
                                })?;
                                let result = if guard.read_shutdown {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP read side is shut down",
                                    ))
                                } else if let Some(socket) = guard.socket.as_mut() {
                                    let mut bytes = vec![0; limit];
                                    socket.read(&mut bytes).map(|count| {
                                        bytes.truncate(count);
                                        Value::List {
                                            capacity: bytes.len(),
                                            values: bytes
                                                .into_iter()
                                                .map(|byte| Value::Unsigned(byte as u128, 8))
                                                .collect(),
                                        }
                                    })
                                } else {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP stream is closed",
                                    ))
                                };
                                Ok(runtime_result(result))
                            }
                            "write" | "write_async" | "write_async_timeout" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let values = match value {
                                    Value::List { values, .. } | Value::Slice(values) => values,
                                    _ => unreachable!("type checking validates TCP bytes"),
                                };
                                let bytes = values
                                    .into_iter()
                                    .map(|value| match value {
                                        Value::Unsigned(byte, 8) => byte as u8,
                                        _ => unreachable!("type checking validates u8 elements"),
                                    })
                                    .collect::<Vec<_>>();
                                if field != "write" {
                                    let timeout = if field == "write_async_timeout" {
                                        let Value::Duration(duration) =
                                            self.evaluate(program, &arguments[1])?
                                        else {
                                            unreachable!("type checking validates write timeout")
                                        };
                                        Some(duration)
                                    } else {
                                        None
                                    };
                                    return Ok(Value::Future(RuntimeFuture::operation(
                                        FutureWork::SocketWrite(stream, bytes, timeout),
                                    )));
                                }
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TCP stream state is poisoned", object.span)
                                })?;
                                let result = if guard.write_shutdown {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP write side is shut down",
                                    ))
                                } else if let Some(socket) = guard.socket.as_mut() {
                                    socket
                                        .write_all(&bytes)
                                        .map(|()| Value::UInt(bytes.len() as u64))
                                } else {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP stream is closed",
                                    ))
                                };
                                Ok(runtime_result(result))
                            }
                            "close" => {
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TCP stream state is poisoned", object.span)
                                })?;
                                if let Some(socket) = guard.socket.take() {
                                    let _ = socket.shutdown(Shutdown::Both);
                                }
                                Ok(Value::Unit)
                            }
                            "shutdown_read" | "shutdown_write" => {
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TCP stream state is poisoned", object.span)
                                })?;
                                let reading = field == "shutdown_read";
                                let already = if reading {
                                    guard.read_shutdown
                                } else {
                                    guard.write_shutdown
                                };
                                let result = if already {
                                    Ok(Value::Unit)
                                } else if let Some(socket) = guard.socket.as_mut() {
                                    let result = socket
                                        .shutdown(if reading {
                                            Shutdown::Read
                                        } else {
                                            Shutdown::Write
                                        })
                                        .map(|()| Value::Unit);
                                    if result.is_ok() {
                                        if reading {
                                            guard.read_shutdown = true;
                                        } else {
                                            guard.write_shutdown = true;
                                        }
                                    }
                                    result
                                } else {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP stream is closed",
                                    ))
                                };
                                Ok(runtime_result(result))
                            }
                            _ => Err(self.error("unknown TcpStream operation", expression.span)),
                        };
                    }
                    if let Value::TlsStream(stream) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "read" | "read_async" | "read_async_timeout" => {
                                let limit = self.index_value(program, &arguments[0])?;
                                if limit > 16 * 1024 * 1024 {
                                    return Err(self.error(
                                        "TLS read limit exceeds the 16 MiB safety limit",
                                        arguments[0].span,
                                    ));
                                }
                                if field != "read" {
                                    let timeout = if field == "read_async_timeout" {
                                        let Value::Duration(duration) =
                                            self.evaluate(program, &arguments[1])?
                                        else {
                                            unreachable!("type checking validates TLS timeout")
                                        };
                                        Some(duration)
                                    } else {
                                        None
                                    };
                                    return Ok(Value::Future(RuntimeFuture::operation(
                                        FutureWork::TlsRead(stream, limit, timeout),
                                    )));
                                }
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TLS stream state is poisoned", object.span)
                                })?;
                                let result = if let Some(stream) = guard.as_mut() {
                                    let mut bytes = vec![0; limit];
                                    stream.read(&mut bytes).map(|count| {
                                        bytes.truncate(count);
                                        runtime_bytes(bytes)
                                    })
                                } else {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TLS stream is closed",
                                    ))
                                };
                                Ok(runtime_result(result))
                            }
                            "write" | "write_async" | "write_async_timeout" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let values = match value {
                                    Value::List { values, .. } | Value::Slice(values) => values,
                                    _ => unreachable!("type checking validates TLS bytes"),
                                };
                                let bytes = values
                                    .into_iter()
                                    .map(|value| match value {
                                        Value::Unsigned(byte, 8) => byte as u8,
                                        _ => unreachable!("type checking validates u8 elements"),
                                    })
                                    .collect::<Vec<_>>();
                                if field != "write" {
                                    let timeout = if field == "write_async_timeout" {
                                        let Value::Duration(duration) =
                                            self.evaluate(program, &arguments[1])?
                                        else {
                                            unreachable!("type checking validates TLS timeout")
                                        };
                                        Some(duration)
                                    } else {
                                        None
                                    };
                                    return Ok(Value::Future(RuntimeFuture::operation(
                                        FutureWork::TlsWrite(stream, bytes, timeout),
                                    )));
                                }
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TLS stream state is poisoned", object.span)
                                })?;
                                let result = if let Some(stream) = guard.as_mut() {
                                    stream
                                        .write_all(&bytes)
                                        .map(|()| Value::UInt(bytes.len() as u64))
                                } else {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TLS stream is closed",
                                    ))
                                };
                                Ok(runtime_result(result))
                            }
                            "close" => {
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TLS stream state is poisoned", object.span)
                                })?;
                                if let Some(mut secure) = guard.take() {
                                    let _ = secure.shutdown();
                                    let _ = secure.get_ref().shutdown(Shutdown::Both);
                                }
                                Ok(Value::Unit)
                            }
                            _ => Err(self.error("unknown TlsStream operation", expression.span)),
                        };
                    }
                    if let Value::HttpResponse(response) = self.evaluate(program, object)? {
                        let response = &response.0;
                        return match field.as_str() {
                            "status" => Ok(Value::UInt(response.status as u64)),
                            "is_success" => Ok(Value::Bool((200..300).contains(&response.status))),
                            "body" => Ok(runtime_bytes(response.body.clone())),
                            "text" => Ok(match String::from_utf8(response.body.clone()) {
                                Ok(text) => {
                                    runtime_result(Ok(Value::String(RuntimeString::literal(text))))
                                }
                                Err(error) => runtime_result(Err(http_error(
                                    io::ErrorKind::InvalidData,
                                    format!("HTTP response body is not valid UTF-8: {error}"),
                                ))),
                            }),
                            "json" => Ok(runtime_result(
                                String::from_utf8(response.body.clone())
                                    .map_err(|error| {
                                        http_error(
                                            io::ErrorKind::InvalidData,
                                            format!(
                                                "HTTP response body is not valid UTF-8: {error}"
                                            ),
                                        )
                                    })
                                    .and_then(runtime_json)
                                    .map(Value::Json),
                            )),
                            "header" => {
                                let Value::String(name) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates HTTP header name")
                                };
                                if name.text.is_empty()
                                    || !name.text.bytes().all(|byte| {
                                        byte.is_ascii_alphanumeric()
                                            || matches!(
                                                byte,
                                                b'!' | b'#'
                                                    | b'$'
                                                    | b'%'
                                                    | b'&'
                                                    | b'\''
                                                    | b'*'
                                                    | b'+'
                                                    | b'-'
                                                    | b'.'
                                                    | b'^'
                                                    | b'_'
                                                    | b'`'
                                                    | b'|'
                                                    | b'~'
                                            )
                                    })
                                {
                                    return Err(self.error(
                                        "HTTP header name contains invalid characters",
                                        arguments[0].span,
                                    ));
                                }
                                let values = response
                                    .headers
                                    .iter()
                                    .filter(|(header, _)| header.eq_ignore_ascii_case(&name.text))
                                    .map(|(_, value)| value.as_str())
                                    .collect::<Vec<_>>();
                                Ok(option_value((!values.is_empty()).then(|| {
                                    Value::String(RuntimeString::literal(values.join(", ")))
                                })))
                            }
                            "url" => {
                                Ok(Value::String(RuntimeString::literal(response.url.clone())))
                            }
                            "len" => Ok(Value::UInt(response.body.len() as u64)),
                            "is_empty" => Ok(Value::Bool(response.body.is_empty())),
                            _ => Err(self.error("unknown HttpResponse operation", expression.span)),
                        };
                    }
                    let http_request_receiver = self
                        .expression_place(object)
                        .map(|place| matches!(self.read_place(&place), Some(Value::HttpRequest(_))))
                        .unwrap_or(true);
                    if http_request_receiver
                        && matches!(
                            field.as_str(),
                            "header" | "text" | "bytes" | "json" | "send" | "send_timeout"
                        )
                    {
                        let value = self.consume(program, object)?;
                        if let Value::HttpRequest(mut request) = value.clone() {
                            return match field.as_str() {
                                "header" => {
                                    let Value::String(name) =
                                        self.evaluate(program, &arguments[0])?
                                    else {
                                        unreachable!("type checking validates HTTP header name")
                                    };
                                    let Value::String(value) =
                                        self.evaluate(program, &arguments[1])?
                                    else {
                                        unreachable!("type checking validates HTTP header value")
                                    };
                                    let result = http_header(&name.text, &value.text)
                                        .and_then(|()| {
                                            request.headers.push((name.text, value.text));
                                            http_request_size(&request)
                                        })
                                        .map(|()| Value::HttpRequest(request));
                                    Ok(runtime_result(result))
                                }
                                "text" | "bytes" | "json" => {
                                    let body =
                                        runtime_http_body(self.evaluate(program, &arguments[0])?)
                                            .unwrap_or_else(|| {
                                                unreachable!("type checking validates HTTP body")
                                            });
                                    request.body = body;
                                    if matches!(field.as_str(), "text" | "json")
                                        && !request.headers.iter().any(|(name, _)| {
                                            name.eq_ignore_ascii_case("content-type")
                                        })
                                    {
                                        request.headers.push((
                                            "Content-Type".into(),
                                            if field == "json" {
                                                "application/json".into()
                                            } else {
                                                "text/plain; charset=utf-8".into()
                                            },
                                        ));
                                    }
                                    Ok(runtime_result(
                                        http_request_size(&request)
                                            .map(|()| Value::HttpRequest(request)),
                                    ))
                                }
                                "send" | "send_timeout" => {
                                    let timeout = if field == "send_timeout" {
                                        let Value::Duration(duration) =
                                            self.evaluate(program, &arguments[0])?
                                        else {
                                            unreachable!("type checking validates HTTP timeout")
                                        };
                                        duration
                                    } else {
                                        StdDuration::from_secs(30)
                                    };
                                    Ok(Value::Future(RuntimeFuture::operation(
                                        FutureWork::HttpRequest(request, timeout),
                                    )))
                                }
                                _ => unreachable!(),
                            };
                        }
                        if let Value::UdpDatagram(datagram) = value
                            && field == "bytes"
                        {
                            return Ok(Value::List {
                                capacity: datagram.bytes.len(),
                                values: datagram
                                    .bytes
                                    .into_iter()
                                    .map(|byte| Value::Unsigned(byte as u128, 8))
                                    .collect(),
                            });
                        }
                    }
                    if let Value::UdpSocket(socket) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "receive_from"
                            | "receive_from_async"
                            | "receive_from_async_timeout" => {
                                let limit = self.index_value(program, &arguments[0])?;
                                if limit > 65_535 {
                                    return Err(self.error(
                                        "UDP receive limit exceeds 65535 bytes",
                                        arguments[0].span,
                                    ));
                                }
                                let timeout = if field == "receive_from_async_timeout" {
                                    let Value::Duration(duration) =
                                        self.evaluate(program, &arguments[1])?
                                    else {
                                        unreachable!("type checking validates UDP timeout")
                                    };
                                    Some(duration)
                                } else {
                                    None
                                };
                                let future = RuntimeFuture::operation(FutureWork::UdpReceive(
                                    socket, limit, timeout,
                                ));
                                if field == "receive_from" {
                                    self.await_future(program, future, expression.span)
                                } else {
                                    Ok(Value::Future(future))
                                }
                            }
                            "send_to" | "send_to_async" | "send_to_async_timeout" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let values = match value {
                                    Value::List { values, .. } | Value::Slice(values) => values,
                                    _ => unreachable!("type checking validates UDP bytes"),
                                };
                                let bytes = values
                                    .into_iter()
                                    .map(|value| match value {
                                        Value::Unsigned(byte, 8) => byte as u8,
                                        _ => unreachable!("type checking validates u8 elements"),
                                    })
                                    .collect::<Vec<_>>();
                                let Value::SocketAddress(address) =
                                    self.evaluate(program, &arguments[1])?
                                else {
                                    unreachable!("type checking validates UDP address")
                                };
                                let timeout = if field == "send_to_async_timeout" {
                                    let Value::Duration(duration) =
                                        self.evaluate(program, &arguments[2])?
                                    else {
                                        unreachable!("type checking validates UDP timeout")
                                    };
                                    Some(duration)
                                } else {
                                    None
                                };
                                let future = RuntimeFuture::operation(FutureWork::UdpSend(
                                    socket, bytes, address, timeout,
                                ));
                                if field == "send_to" {
                                    self.await_future(program, future, expression.span)
                                } else {
                                    Ok(Value::Future(future))
                                }
                            }
                            "local_port" => {
                                let guard = socket.0.lock().map_err(|_| {
                                    self.error("UDP socket state is poisoned", object.span)
                                })?;
                                let port = match guard.as_ref() {
                                    Some(socket) => socket
                                        .local_addr()
                                        .map(|address| Value::UInt(address.port() as u64)),
                                    None => Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "UDP socket is closed",
                                    )),
                                };
                                Ok(runtime_result(port))
                            }
                            "close" => {
                                let mut guard = socket.0.lock().map_err(|_| {
                                    self.error("UDP socket state is poisoned", object.span)
                                })?;
                                guard.take();
                                Ok(Value::Unit)
                            }
                            _ => Err(self.error("unknown UdpSocket operation", expression.span)),
                        };
                    }
                    if let Value::UdpDatagram(datagram) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "bytes" => Ok(Value::List {
                                capacity: datagram.bytes.len(),
                                values: datagram
                                    .bytes
                                    .into_iter()
                                    .map(|byte| Value::Unsigned(byte as u128, 8))
                                    .collect(),
                            }),
                            "source" => Ok(Value::SocketAddress(datagram.source)),
                            "len" => Ok(Value::UInt(datagram.bytes.len() as u64)),
                            "is_empty" => Ok(Value::Bool(datagram.bytes.is_empty())),
                            _ => Err(self.error("unknown UdpDatagram operation", expression.span)),
                        };
                    }
                    if let Value::IpAddress(address) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "as_string" => {
                                Ok(Value::String(RuntimeString::literal(address.0.to_string())))
                            }
                            "is_ipv4" => Ok(Value::Bool(address.0.is_ipv4())),
                            "is_ipv6" => Ok(Value::Bool(address.0.is_ipv6())),
                            "is_loopback" => Ok(Value::Bool(address.0.is_loopback())),
                            "is_unspecified" => Ok(Value::Bool(address.0.is_unspecified())),
                            _ => Err(self.error("unknown IpAddress operation", expression.span)),
                        };
                    }
                    if let Value::Path(path) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "join" => {
                                let child = runtime_path(self.evaluate(program, &arguments[0])?)
                                    .ok_or_else(|| {
                                        self.error(
                                            "Path.join expects Path, String, or str",
                                            arguments[0].span,
                                        )
                                    })?;
                                Ok(Value::Path(path.join(child)))
                            }
                            "len" => {
                                Ok(Value::UInt(path.as_os_str().to_string_lossy().len() as u64))
                            }
                            "is_empty" => Ok(Value::Bool(path.as_os_str().is_empty())),
                            "is_absolute" => Ok(Value::Bool(path.is_absolute())),
                            "as_string" => Ok(Value::String(RuntimeString::literal(
                                path.to_string_lossy().into_owned(),
                            ))),
                            "name" => Ok(option_value(path.file_name().map(|value| {
                                Value::String(RuntimeString::literal(
                                    value.to_string_lossy().into_owned(),
                                ))
                            }))),
                            "extension" => Ok(option_value(path.extension().map(|value| {
                                Value::String(RuntimeString::literal(
                                    value.to_string_lossy().into_owned(),
                                ))
                            }))),
                            "parent" => Ok(option_value(
                                path.parent().map(|value| Value::Path(value.to_path_buf())),
                            )),
                            _ => Err(self.error("unknown Path operation", expression.span)),
                        };
                    }
                    if matches!(
                        field.as_str(),
                        "execute"
                            | "query"
                            | "begin"
                            | "commit"
                            | "rollback"
                            | "close"
                            | "changes"
                            | "last_insert_id"
                    ) {
                        let database = if field == "close" {
                            self.consume(program, object)?
                        } else {
                            self.evaluate(program, object)?
                        };
                        if let Value::Database(database) = database {
                            if matches!(field.as_str(), "execute" | "query") {
                                let Value::String(sql) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates database SQL")
                                };
                                let Value::List { values, .. } =
                                    self.evaluate(program, &arguments[1])?
                                else {
                                    unreachable!("type checking validates database parameters")
                                };
                                let parameters = values
                                    .into_iter()
                                    .map(|value| match value {
                                        Value::Json(json) => json,
                                        _ => unreachable!(
                                            "type checking validates database parameters"
                                        ),
                                    })
                                    .collect::<Vec<_>>();
                                return Ok(if field == "execute" {
                                    runtime_result(
                                        database.execute(&sql.text, &parameters).map(Value::UInt),
                                    )
                                } else {
                                    runtime_result(database.query(&sql.text, &parameters).map(
                                        |rows| {
                                            let capacity = rows.len();
                                            Value::List {
                                                values: rows.into_iter().map(Value::Json).collect(),
                                                capacity,
                                            }
                                        },
                                    ))
                                });
                            }
                            return match field.as_str() {
                                "begin" => Ok(runtime_result(
                                    database
                                        .control(b"BEGIN IMMEDIATE\0", false)
                                        .map(|()| Value::Unit),
                                )),
                                "commit" => Ok(runtime_result(
                                    database.control(b"COMMIT\0", true).map(|()| Value::Unit),
                                )),
                                "rollback" => Ok(runtime_result(
                                    database.control(b"ROLLBACK\0", true).map(|()| Value::Unit),
                                )),
                                "close" => {
                                    Ok(runtime_result(database.close().map(|()| Value::Unit)))
                                }
                                "changes" => database.changes().map(Value::UInt).map_err(|error| {
                                    self.error(error.to_string(), expression.span)
                                }),
                                "last_insert_id" => {
                                    database.last_insert_id().map(Value::Int).map_err(|error| {
                                        self.error(error.to_string(), expression.span)
                                    })
                                }
                                _ => unreachable!(),
                            };
                        }
                    }
                    if let Value::ProcessOutput(output) = self.evaluate(program, object)? {
                        let bytes = |values: Vec<u8>| Value::List {
                            capacity: values.len(),
                            values: values
                                .into_iter()
                                .map(|value| Value::Unsigned(value as u128, 8))
                                .collect(),
                        };
                        return match field.as_str() {
                            "status" => Ok(Value::Int(output.status)),
                            "success" => Ok(Value::Bool(output.status == 0)),
                            "stdout" => Ok(bytes(output.stdout)),
                            "stderr" => Ok(bytes(output.stderr)),
                            "stdout_text" | "stderr_text" => {
                                let raw = if field == "stdout_text" {
                                    output.stdout
                                } else {
                                    output.stderr
                                };
                                Ok(runtime_result(
                                    String::from_utf8(raw)
                                        .map(|text| Value::String(RuntimeString::literal(text)))
                                        .map_err(|_| {
                                            json_conversion_error(
                                                "process output is not valid UTF-8",
                                            )
                                        }),
                                ))
                            }
                            _ => {
                                Err(self.error("unknown ProcessOutput operation", expression.span))
                            }
                        };
                    }
                    if matches!(
                        field.as_str(),
                        "arg"
                            | "arguments"
                            | "directory"
                            | "environment"
                            | "clear_environment"
                            | "input"
                            | "input_text"
                            | "timeout"
                            | "start"
                            | "run"
                    ) {
                        let command = if let Some(place) = self.expression_place(object)
                            && matches!(self.read_place(&place), Some(Value::ProcessCommand(_)))
                        {
                            Some(self.consume(program, object)?)
                        } else {
                            match self.evaluate(program, object)? {
                                value @ Value::ProcessCommand(_) => Some(value),
                                _ => None,
                            }
                        };
                        if let Some(Value::ProcessCommand(mut command)) = command {
                            match field.as_str() {
                                "arg" => {
                                    let Value::String(value) =
                                        self.consume(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    command.arguments.push(value.text);
                                }
                                "arguments" => {
                                    let Value::List { values, .. } =
                                        self.consume(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    command.arguments.extend(values.into_iter().map(|value| {
                                        let Value::String(value) = value else {
                                            unreachable!()
                                        };
                                        value.text
                                    }));
                                }
                                "directory" => {
                                    let Value::Path(value) =
                                        self.consume(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    command.directory = Some(value);
                                }
                                "environment" => {
                                    let Value::String(name) =
                                        self.consume(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    let Value::String(value) =
                                        self.consume(program, &arguments[1])?
                                    else {
                                        unreachable!()
                                    };
                                    command
                                        .environment
                                        .retain(|(existing, _)| existing != &name.text);
                                    command.environment.push((name.text, value.text));
                                }
                                "clear_environment" => command.clear_environment = true,
                                "input" => {
                                    let Value::List { values, .. } =
                                        self.consume(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    command.input = values
                                        .into_iter()
                                        .map(|value| {
                                            let Value::Unsigned(value, 8) = value else {
                                                unreachable!()
                                            };
                                            value as u8
                                        })
                                        .collect();
                                }
                                "input_text" => {
                                    let Value::String(value) =
                                        self.consume(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    command.input = value.text.into_bytes();
                                }
                                "timeout" => {
                                    let Value::Duration(value) =
                                        self.evaluate(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    command.timeout = Some(value);
                                }
                                "run" => return Ok(runtime_result(execute_process(*command))),
                                "start" => return Ok(runtime_result(start_process(*command))),
                                _ => {
                                    return Err(self.error(
                                        "unknown ProcessCommand operation",
                                        expression.span,
                                    ));
                                }
                            }
                            return Ok(Value::ProcessCommand(command));
                        }
                    }
                    if matches!(
                        field.as_str(),
                        "write"
                            | "write_text"
                            | "close_input"
                            | "read_stdout"
                            | "read_stderr"
                            | "try_wait"
                            | "kill"
                            | "wait"
                    ) {
                        let child = if field == "wait" {
                            self.consume(program, object)?
                        } else {
                            self.evaluate(program, object)?
                        };
                        if let Value::ChildProcess(child) = child {
                            let result = match field.as_str() {
                                "write" => {
                                    let bytes =
                                        runtime_http_body(self.evaluate(program, &arguments[0])?)
                                            .expect("type checking validates child-process bytes");
                                    child.write(&bytes).map(|()| Value::Unit)
                                }
                                "write_text" => {
                                    let Value::String(text) =
                                        self.evaluate(program, &arguments[0])?
                                    else {
                                        unreachable!()
                                    };
                                    child.write(text.text.as_bytes()).map(|()| Value::Unit)
                                }
                                "close_input" => child.close_input().map(|()| Value::Unit),
                                "read_stdout" | "read_stderr" => {
                                    let limit = self.index_value(program, &arguments[0])?;
                                    child
                                        .read_pipe(field == "read_stdout", limit)
                                        .map(runtime_bytes)
                                }
                                "try_wait" => child
                                    .try_wait()
                                    .map(|status| option_value(status.map(Value::Int))),
                                "kill" => child.kill().map(|()| Value::Unit),
                                "wait" => child.wait(),
                                _ => unreachable!(),
                            };
                            return Ok(runtime_result(result));
                        }
                    }
                    if let Value::Url(url) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "as_string" => {
                                Ok(Value::String(RuntimeString::literal(url.text.clone())))
                            }
                            "scheme" => Ok(Value::String(RuntimeString::literal(
                                url.scheme().to_owned(),
                            ))),
                            "host" => Ok(option_value(url.host().map(|host| {
                                Value::String(RuntimeString::literal(host.to_owned()))
                            }))),
                            "port" => Ok(option_value(
                                url.port().map(|port| Value::UInt(u64::from(port))),
                            )),
                            "path" => {
                                Ok(Value::String(RuntimeString::literal(url.path().to_owned())))
                            }
                            "query" => Ok(option_value(url.query().map(|query| {
                                Value::String(RuntimeString::literal(query.to_owned()))
                            }))),
                            "is_secure" => {
                                Ok(Value::Bool(url.scheme().eq_ignore_ascii_case("https")))
                            }
                            "join_path" => {
                                let Value::String(segment) =
                                    self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates Url.join_path")
                                };
                                Ok(runtime_result(url.join_path(&segment.text).map(Value::Url)))
                            }
                            "query_param" => {
                                let Value::String(name) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates Url.query_param name")
                                };
                                let Value::String(value) = self.evaluate(program, &arguments[1])?
                                else {
                                    unreachable!("type checking validates Url.query_param value")
                                };
                                Ok(runtime_result(
                                    url.query_param(&name.text, &value.text).map(Value::Url),
                                ))
                            }
                            _ => Err(self.error("unknown Url operation", expression.span)),
                        };
                    }
                    if let Value::Json(json) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "as_string" => Ok(Value::String(RuntimeString::literal(json.text))),
                            "kind" => Ok(Value::String(RuntimeString::literal(json.kind.into()))),
                            "len" => Ok(Value::UInt(json.text.len() as u64)),
                            "is_null" => Ok(Value::Bool(json.kind == "null")),
                            "is_bool" => Ok(Value::Bool(json.kind == "bool")),
                            "is_number" => Ok(Value::Bool(json.kind == "number")),
                            "is_string" => Ok(Value::Bool(json.kind == "string")),
                            "is_array" => Ok(Value::Bool(json.kind == "array")),
                            "is_object" => Ok(Value::Bool(json.kind == "object")),
                            "get" => {
                                let Value::String(key) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates Json.get key")
                                };
                                let value = json_get(&json, &key.text).map_err(|error| {
                                    self.error(error.to_string(), expression.span)
                                })?;
                                Ok(option_value(value.map(Value::Json)))
                            }
                            "at" => {
                                let index = self.index_value(program, &arguments[0])?;
                                let value = json_at(&json, index).map_err(|error| {
                                    self.error(error.to_string(), expression.span)
                                })?;
                                Ok(option_value(value.map(Value::Json)))
                            }
                            "as_bool" => Ok(runtime_result(match json.text.trim() {
                                "true" => Ok(Value::Bool(true)),
                                "false" => Ok(Value::Bool(false)),
                                _ => Err(json_conversion_error("JSON value is not a bool")),
                            })),
                            "as_int" => Ok(runtime_result(
                                json.text
                                    .trim()
                                    .parse::<i64>()
                                    .map(Value::Int)
                                    .map_err(|_| {
                                        json_conversion_error(
                                            "JSON value is not an integer representable as int",
                                        )
                                    }),
                            )),
                            "as_uint" => Ok(runtime_result(
                                json.text
                                    .trim()
                                    .parse::<u64>()
                                    .map(Value::UInt)
                                    .map_err(|_| {
                                        json_conversion_error(
                                            "JSON value is not an integer representable as uint",
                                        )
                                    }),
                            )),
                            "as_f64" => Ok(runtime_result(if json.kind != "number" {
                                Err(json_conversion_error("JSON value is not a number"))
                            } else {
                                json.text
                                    .trim()
                                    .parse::<f64>()
                                    .ok()
                                    .filter(|value| value.is_finite())
                                    .map(Value::Float)
                                    .ok_or_else(|| {
                                        json_conversion_error(
                                            "JSON number is not representable as f64",
                                        )
                                    })
                            })),
                            "as_text" => Ok(runtime_result(
                                json_string_value(json.text.trim())
                                    .map(|text| Value::String(RuntimeString::literal(text))),
                            )),
                            _ => Err(self.error("unknown Json operation", expression.span)),
                        };
                    }
                    if let Value::Instant(instant) = self.evaluate(program, object)?
                        && field == "elapsed"
                    {
                        return Ok(Value::Duration(instant.elapsed()));
                    }
                    if let Value::Duration(duration) = self.evaluate(program, object)? {
                        return Ok(Value::UInt(match field.as_str() {
                            "nanos" => duration.as_nanos().min(u64::MAX as u128) as u64,
                            "millis" => duration.as_millis().min(u64::MAX as u128) as u64,
                            _ => duration.as_secs(),
                        }));
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Map {
                            mut entries,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let method = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains_key" => "has",
                            "insert" => "set",
                            other => other,
                        };
                        match method {
                            "keys" => {
                                return Ok(Value::Slice(
                                    entries.iter().map(|(key, _)| key.clone()).collect(),
                                ));
                            }
                            "values" => {
                                return Ok(Value::Slice(
                                    entries.iter().map(|(_, value)| value.clone()).collect(),
                                ));
                            }
                            "len" => return Ok(Value::UInt(entries.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(entries.is_empty())),
                            "has" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(
                                    entries.iter().any(|(candidate, _)| candidate == &key),
                                ));
                            }
                            "get" | "get_mut" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(
                                    match entries
                                        .iter()
                                        .position(|(candidate, _)| candidate == &key)
                                    {
                                        Some(index) => {
                                            let mut value = place.clone();
                                            value.fields.push(PlaceSegment::MapValue(index));
                                            Value::Enum {
                                                type_name: "Option".into(),
                                                variant: "Some".into(),
                                                payload: vec![Value::Reference(
                                                    value,
                                                    method == "get_mut",
                                                )],
                                            }
                                        }
                                        None => Value::Enum {
                                            type_name: "Option".into(),
                                            variant: "None".into(),
                                            payload: vec![],
                                        },
                                    },
                                );
                            }
                            "set" => {
                                let key = self.consume(program, &arguments[0])?;
                                let value = self.consume(program, &arguments[1])?;
                                let old = if let Some(entry) =
                                    entries.iter_mut().find(|(candidate, _)| candidate == &key)
                                {
                                    Some(std::mem::replace(&mut entry.1, value))
                                } else {
                                    grow_list_capacity(&mut capacity, entries.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    entries.push((key, value));
                                    None
                                };
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(option_value(old));
                            }
                            "remove" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                let removed = entries
                                    .iter()
                                    .position(|(candidate, _)| candidate == &key)
                                    .map(|index| entries.remove(index).1);
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(option_value(removed));
                            }
                            "clear" => entries.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Map { entries, capacity })
                            .ok_or_else(|| {
                                self.error("Map mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Set {
                            mut values,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let method = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains" => "has",
                            "insert" => "add",
                            other => other,
                        };
                        match method {
                            "iter" => return Ok(Value::Slice(values)),
                            "len" => return Ok(Value::UInt(values.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(values.is_empty())),
                            "has" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(values.contains(&value)));
                            }
                            "add" => {
                                let value = self.consume(program, &arguments[0])?;
                                let added = if values.contains(&value) {
                                    false
                                } else {
                                    grow_list_capacity(&mut capacity, values.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    values.push(value);
                                    true
                                };
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(added));
                            }
                            "remove" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let removed = values
                                    .iter()
                                    .position(|candidate| candidate == &value)
                                    .map(|index| values.remove(index))
                                    .is_some();
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(removed));
                            }
                            "clear" => values.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Set { values, capacity })
                            .ok_or_else(|| {
                                self.error("Set mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::List {
                            mut values,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let field = match field.as_str() {
                            "add" => "push",
                            "count" => "len",
                            "empty" => "is_empty",
                            other => other,
                        };
                        match field {
                            "iter" => return Ok(Value::Slice(values)),
                            "len" if arguments.is_empty() => {
                                return Ok(Value::UInt(values.len() as u64));
                            }
                            "capacity" if arguments.is_empty() => {
                                return Ok(Value::UInt(capacity as u64));
                            }
                            "is_empty" if arguments.is_empty() => {
                                return Ok(Value::Bool(values.is_empty()));
                            }
                            "push" => {
                                let value = self.consume(program, &arguments[0])?;
                                grow_list_capacity(&mut capacity, values.len() + 1)
                                    .map_err(|message| self.error(message, expression.span))?;
                                values.push(value);
                            }
                            "pop" => {
                                let value = values.pop();
                                self.write_place(&place, Value::List { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("List mutation target is invalid", object.span)
                                    })?;
                                return Ok(match value {
                                    Some(value) => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "Some".into(),
                                        payload: vec![value],
                                    },
                                    None => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    },
                                });
                            }
                            "get" | "get_mut" => {
                                let index = self.index_value(program, &arguments[0])?;
                                if index >= values.len() {
                                    return Ok(Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    });
                                }
                                let mut element = place.clone();
                                element.fields.push(PlaceSegment::Index(index));
                                return Ok(Value::Enum {
                                    type_name: "Option".into(),
                                    variant: "Some".into(),
                                    payload: vec![Value::Reference(element, field == "get_mut")],
                                });
                            }
                            "insert" => {
                                let index = self.index_value(program, &arguments[0])?;
                                if index > values.len() {
                                    return Err(self.error(
                                        format!(
                                            "List insertion index {index} is out of bounds for length {}",
                                            values.len()
                                        ),
                                        arguments[0].span,
                                    ));
                                }
                                let value = self.consume(program, &arguments[1])?;
                                grow_list_capacity(&mut capacity, values.len() + 1)
                                    .map_err(|message| self.error(message, expression.span))?;
                                values.insert(index, value);
                            }
                            "remove" => {
                                let index = self.index_value(program, &arguments[0])?;
                                if index >= values.len() {
                                    return Err(self.error(
                                        format!(
                                            "List removal index {index} is out of bounds for length {}",
                                            values.len()
                                        ),
                                        arguments[0].span,
                                    ));
                                }
                                let removed = values.remove(index);
                                self.write_place(&place, Value::List { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("List mutation target is invalid", object.span)
                                    })?;
                                return Ok(removed);
                            }
                            "clear" => values.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::List { values, capacity })
                            .ok_or_else(|| {
                                self.error("List mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if matches!(field.as_str(), "len" | "is_empty")
                        && arguments.is_empty()
                        && let Value::Array(values) | Value::Slice(values) =
                            self.evaluate(program, object)?
                    {
                        return Ok(if field == "is_empty" {
                            Value::Bool(values.is_empty())
                        } else {
                            Value::UInt(values.len() as u64)
                        });
                    }
                    if field == "iter"
                        && arguments.is_empty()
                        && let Value::Array(values) | Value::Slice(values) =
                            self.evaluate(program, object)?
                    {
                        return Ok(Value::Slice(values));
                    }
                    if matches!(field.as_str(), "as_ptr" | "as_mut_ptr")
                        && arguments.is_empty()
                        && self
                            .dynamic_place(program, object)?
                            .and_then(|place| self.read_place(&place))
                            .is_some_and(|value| matches!(value, Value::Memory(_)))
                    {
                        let mut place = self.dynamic_place(program, object)?.ok_or_else(|| {
                            self.error("Memory pointer source is not a place", object.span)
                        })?;
                        place.fields.push(PlaceSegment::Index(0));
                        return Ok(Value::Reference(place, field == "as_mut_ptr"));
                    }
                    if matches!(
                        field.as_str(),
                        "len" | "alignment" | "is_empty" | "read" | "write" | "fill" | "copy_from"
                    ) && let Value::Memory(memory) = self.evaluate(program, object)?
                    {
                        let length = memory.len().ok_or_else(|| {
                            self.error("Memory state lock is poisoned", object.span)
                        })?;
                        return match field.as_str() {
                            "len" => Ok(Value::UInt(length as u64)),
                            "alignment" => Ok(Value::UInt(memory.alignment().ok_or_else(|| {
                                self.error("Memory state lock is poisoned", object.span)
                            })? as u64)),
                            "is_empty" => Ok(Value::Bool(length == 0)),
                            "read" => {
                                let index = self.index_value(program, &arguments[0])?;
                                memory
                                    .read(index)
                                    .map(|value| Value::Unsigned(value.into(), 8))
                                    .ok_or_else(|| {
                                        self.error(
                                            format!(
                                                "Memory index {index} is out of bounds for length {length}"
                                            ),
                                            arguments[0].span,
                                        )
                                    })
                            }
                            "write" => {
                                let index = self.index_value(program, &arguments[0])?;
                                let byte = runtime_byte(self.evaluate(program, &arguments[1])?)
                                    .ok_or_else(|| {
                                        self.error("Memory value must be a u8", arguments[1].span)
                                    })?;
                                memory.write(index, byte).ok_or_else(|| {
                                    self.error(
                                        format!(
                                            "Memory index {index} is out of bounds for length {length}"
                                        ),
                                        arguments[0].span,
                                    )
                                })?;
                                Ok(Value::Unit)
                            }
                            "fill" => {
                                let byte = runtime_byte(self.evaluate(program, &arguments[0])?)
                                    .ok_or_else(|| {
                                        self.error("Memory value must be a u8", arguments[0].span)
                                    })?;
                                memory.fill(byte).ok_or_else(|| {
                                    self.error("Memory state lock is poisoned", object.span)
                                })?;
                                Ok(Value::Unit)
                            }
                            "copy_from" => {
                                let destination = self.index_value(program, &arguments[0])?;
                                let Value::Memory(source) =
                                    self.evaluate(program, &arguments[1])?
                                else {
                                    unreachable!("type checking validates Memory copy source")
                                };
                                let source_offset = self.index_value(program, &arguments[2])?;
                                let count = self.index_value(program, &arguments[3])?;
                                memory
                                    .copy_from(destination, &source, source_offset, count)
                                    .ok_or_else(|| {
                                        self.error(
                                            "Memory copy range is out of bounds",
                                            expression.span,
                                        )
                                    })?;
                                Ok(Value::Unit)
                            }
                            _ => unreachable!(),
                        };
                    }
                    if arguments.is_empty()
                        && matches!(
                            field.as_str(),
                            "len" | "is_empty" | "to_string" | "as_c_str"
                        )
                        && let value @ (Value::CString(_) | Value::CStr(_)) =
                            self.evaluate(program, object)?
                    {
                        let (value, owned) = match value {
                            Value::CString(value) => (value, true),
                            Value::CStr(value) => (value, false),
                            _ => unreachable!(),
                        };
                        return Ok(match field.as_str() {
                            "len" => Value::UInt(value.len() as u64),
                            "is_empty" => Value::Bool(value.len() == 0),
                            "to_string" => Value::String(RuntimeString::literal(value.text())),
                            "as_c_str" if owned => Value::CStr(value),
                            _ => unreachable!("type checking validates CString methods"),
                        });
                    }
                    if arguments.is_empty()
                        && matches!(field.as_str(), "len" | "capacity" | "is_empty")
                        && let Value::String(value) = self.evaluate(program, object)?
                    {
                        return Ok(match field.as_str() {
                            "len" => Value::UInt(value.len() as u64),
                            "capacity" => Value::UInt(value.capacity() as u64),
                            _ => Value::Bool(value.is_empty()),
                        });
                    }
                    if matches!(field.as_str(), "contains" | "starts_with" | "ends_with") {
                        let Value::String(value) = self.evaluate(program, object)? else {
                            unreachable!()
                        };
                        let Value::String(pattern) = self.evaluate(program, &arguments[0])? else {
                            unreachable!()
                        };
                        return Ok(Value::Bool(match field.as_str() {
                            "contains" => value.text.contains(&pattern.text),
                            "starts_with" => value.text.starts_with(&pattern.text),
                            _ => value.text.ends_with(&pattern.text),
                        }));
                    }
                    if matches!(
                        field.as_str(),
                        "push" | "push_str" | "append" | "add" | "clear"
                    ) && self
                        .expression_place(object)
                        .and_then(|place| self.read_place(&place))
                        .is_some_and(|value| matches!(value, Value::String(_)))
                    {
                        let place = self.expression_place(object).ok_or_else(|| {
                            self.error("String mutation requires a mutable place", object.span)
                        })?;
                        let Some(Value::String(mut value)) = self.read_place(&place) else {
                            return Err(
                                self.error("String method receiver is invalid", object.span)
                            );
                        };
                        match field.as_str() {
                            "push" => match self.evaluate(program, &arguments[0])? {
                                Value::Char(ch) => value.push(ch),
                                _ => unreachable!(),
                            },
                            "push_str" | "append" | "add" => {
                                match self.evaluate(program, &arguments[0])? {
                                    Value::String(text) => value.push_str(&text),
                                    _ => unreachable!(),
                                }
                            }
                            _ => value.clear(),
                        }
                        self.write_place(&place, Value::String(value))
                            .ok_or_else(|| {
                                self.error("String mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    let receiver = self.evaluate(program, object)?;
                    let receiver_name = value_type_name(&receiver);
                    let methods = program
                        .implementations
                        .iter()
                        .filter_map(|implementation| {
                            (implementation.target.name == receiver_name)
                                .then(|| {
                                    implementation
                                        .methods
                                        .iter()
                                        .find(|method| method.name == *field)
                                })
                                .flatten()
                        })
                        .collect::<Vec<_>>();
                    if methods.len() == 1 {
                        let mut values = Vec::with_capacity(arguments.len() + 1);
                        let qualifier = methods[0]
                            .parameters
                            .first()
                            .map(|parameter| parameter.ty.qualifier)
                            .unwrap_or(crate::ast::TypeQualifier::Owned);
                        if matches!(
                            qualifier,
                            crate::ast::TypeQualifier::SharedReference
                                | crate::ast::TypeQualifier::MutableReference
                        ) {
                            let place = self.expression_place(object).ok_or_else(|| {
                                self.error("method receiver is not borrowable", object.span)
                            })?;
                            values.push(Value::Reference(
                                place,
                                qualifier == crate::ast::TypeQualifier::MutableReference,
                            ));
                        } else {
                            values.push(self.consume(program, object)?);
                        }
                        for (argument, parameter) in
                            arguments.iter().zip(methods[0].parameters.iter().skip(1))
                        {
                            values.push(
                                if parameter.ty.qualifier == crate::ast::TypeQualifier::Owned {
                                    self.consume(program, argument)?
                                } else {
                                    self.evaluate(program, argument)?
                                },
                            );
                        }
                        return self.call_function(program, methods[0], values, expression.span);
                    }
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "print") {
                    let value = self.evaluate(program, &arguments[0])?;
                    let value = match value {
                        Value::Reference(place, _) => self
                            .read_place(&place)
                            .ok_or_else(|| self.error("dangling reference", arguments[0].span))?,
                        value => value,
                    };
                    self.output
                        .lock()
                        .map_err(|_| {
                            self.error("interpreter output lock is poisoned", expression.span)
                        })?
                        .push(display_value(value));
                    return Ok(Value::Unit);
                }
                let callee_value = self.evaluate(program, callee)?;
                if let Value::Constructor { type_name, variant } = callee_value {
                    let mut payload = Vec::with_capacity(arguments.len());
                    for argument in arguments {
                        payload.push(self.consume(program, argument)?);
                    }
                    return Ok(Value::Enum {
                        type_name,
                        variant,
                        payload,
                    });
                }
                if let Value::Closure(closure) = callee_value {
                    let mut values = Vec::with_capacity(arguments.len());
                    for (argument, parameter) in arguments.iter().zip(&closure.parameters) {
                        values.push(
                            if parameter.ty.qualifier == crate::ast::TypeQualifier::Owned {
                                self.consume(program, argument)?
                            } else {
                                self.evaluate(program, argument)?
                            },
                        );
                    }
                    return self.call_closure(program, *closure, values, expression.span);
                }
                let Value::Function(name) = callee_value else {
                    return Err(self.error("expression is not callable", callee.span));
                };
                let mut values = Vec::with_capacity(arguments.len());
                let function = program
                    .functions
                    .iter()
                    .find(|function| function.name == name)
                    .ok_or_else(|| self.error(format!("unknown function `{name}`"), callee.span))?
                    .clone();
                for (argument, parameter) in arguments.iter().zip(&function.parameters) {
                    values.push(
                        if parameter.ty.qualifier == crate::ast::TypeQualifier::Owned {
                            self.consume(program, argument)?
                        } else {
                            let value = self.evaluate(program, argument)?;
                            if matches!(value, Value::Reference(_, _)) {
                                value
                            } else if parameter.ty.qualifier
                                == crate::ast::TypeQualifier::SharedReference
                            {
                                Value::Reference(
                                    self.expression_place(argument).ok_or_else(|| {
                                        self.error(
                                            "implicit borrow argument has no storage place",
                                            argument.span,
                                        )
                                    })?,
                                    false,
                                )
                            } else {
                                value
                            }
                        },
                    );
                }
                self.call_function(program, &function, values, expression.span)
            }
            Expression::Match { value, arms } => {
                let value = self.consume(program, value)?;
                for arm in arms {
                    let mut bindings = HashMap::new();
                    if pattern_matches(&arm.pattern.node, &value, &mut bindings) {
                        self.push_scope(bindings);
                        let result = self.evaluate(program, &arm.value);
                        self.pop_scope();
                        return result;
                    }
                }
                Err(self.error("exhaustive match had no matching arm", expression.span))
            }
            Expression::Try(operand) => {
                let value = self.consume(program, operand)?;
                match &value {
                    Value::Enum {
                        type_name,
                        variant,
                        payload,
                    } if type_name == "Option" && variant == "Some" => Ok(payload[0].clone()),
                    Value::Enum {
                        type_name, variant, ..
                    } if type_name == "Option" && variant == "None" => {
                        Err(RuntimeFault::Propagate(value))
                    }
                    Value::Enum {
                        type_name,
                        variant,
                        payload,
                    } if type_name == "Result" && variant == "Ok" => Ok(payload[0].clone()),
                    Value::Enum {
                        type_name, variant, ..
                    } if type_name == "Result" && variant == "Err" => {
                        Err(RuntimeFault::Propagate(value))
                    }
                    _ => Err(self.error("`?` requires Option or Result", expression.span)),
                }
            }
            Expression::Spawn(task) => self.spawn_task(program, task, expression.span),
            Expression::Await(future) => {
                let value = self.consume(program, future)?;
                match value {
                    Value::Future(future) => self.await_future(program, future, expression.span),
                    Value::Task(task) => {
                        let work = {
                            let mut work = task
                                .0
                                .lock()
                                .map_err(|_| self.error("task state is poisoned", future.span))?;
                            std::mem::replace(&mut *work, RuntimeTaskWork::Consumed)
                        };
                        match work {
                            RuntimeTaskWork::Future(future) => {
                                self.await_future(program, future, expression.span)
                            }
                            RuntimeTaskWork::Ready(value) => Ok(value),
                            RuntimeTaskWork::Running => {
                                Err(self.error("task cannot await itself", future.span))
                            }
                            RuntimeTaskWork::Consumed => {
                                Err(self.error("task has already been awaited", future.span))
                            }
                        }
                    }
                    _ => Err(self.error("`await` requires a Future or Task", future.span)),
                }
            }
            Expression::Borrow { mutable, target } => {
                let place = self
                    .dynamic_place(program, target)?
                    .ok_or_else(|| self.error("borrow target is not a place", target.span))?;
                Ok(Value::Reference(place, *mutable))
            }
            Expression::Dereference(target) => match self.evaluate(program, target)? {
                Value::Reference(place, _) => self
                    .read_place(&place)
                    .ok_or_else(|| self.error("dangling or invalid pointer", expression.span)),
                Value::MutexGuard(guard) => guard
                    .read()
                    .ok_or_else(|| self.error("invalid Mutex guard", expression.span)),
                _ => Err(self.error("value cannot be dereferenced", expression.span)),
            },
            Expression::Move(target) => {
                let place = self
                    .dynamic_place(program, target)?
                    .ok_or_else(|| self.error("move target is not a place", target.span))?;
                let value = self
                    .read_place(&place)
                    .ok_or_else(|| self.error("invalid move target", target.span))?;
                self.write_place(&place, Value::Uninitialized)
                    .ok_or_else(|| self.error("invalid move target", target.span))?;
                Ok(value)
            }
        }
    }

    fn spawn_task(&mut self, program: &Program, task: &Expr, span: Span) -> RuntimeResult<Value> {
        let Expression::Call { callee, arguments } = &task.node else {
            return Err(self.error("`spawn` requires a direct function call", task.span));
        };
        let Expression::Identifier(name) = &callee.node else {
            return Err(self.error("`spawn` requires a named DISP function", callee.span));
        };
        let function = program
            .functions
            .iter()
            .find(|function| function.name == *name)
            .ok_or_else(|| self.error("unknown spawned function", callee.span))?
            .clone();
        let mut values = Vec::with_capacity(arguments.len());
        for argument in arguments {
            values.push(self.consume(program, argument)?);
        }
        let child_program = program.clone();
        let output = Arc::clone(&self.output);
        let program_arguments = self.program_arguments.clone();
        let call_span = task.span;
        let handle = thread::Builder::new()
            .name(format!("disp-{name}"))
            .stack_size(INTERPRETER_STACK_BYTES)
            .spawn(move || {
                let mut child = Interpreter {
                    scopes: Vec::new(),
                    scope_orders: Vec::new(),
                    output,
                    call_depth: 0,
                    tasks: Vec::new(),
                    http_pool: HashMap::new(),
                    program_arguments,
                };
                child
                    .call_function(&child_program, &function, values, call_span)
                    .map(|value| Box::new(value) as Box<dyn Any + Send>)
                    .map_err(RuntimeFault::into_diagnostic)
            })
            .map_err(|error| self.error(format!("could not spawn thread: {error}"), span))?;
        Ok(Value::Thread(RuntimeThread::new(handle)))
    }

    fn push_scope(&mut self, values: HashMap<String, Value>) {
        let order = values.keys().cloned().collect();
        self.scopes.push(values);
        self.scope_orders.push(order);
    }

    fn consume(&mut self, program: &Program, expression: &Expr) -> RuntimeResult<Value> {
        if matches!(expression.node, Expression::Move(_)) {
            return self.evaluate(program, expression);
        }
        if let Some(place) = self.expression_place(expression) {
            let value = self
                .read_place(&place)
                .ok_or_else(|| self.error("invalid move source", expression.span))?;
            if !value_is_copy(program, &value) {
                self.write_place(&place, Value::Uninitialized)
                    .ok_or_else(|| self.error("invalid move source", expression.span))?;
            }
            Ok(value)
        } else {
            self.evaluate(program, expression)
        }
    }

    fn pop_scope(&mut self) {
        if let (Some(scope), Some(order)) = (self.scopes.last_mut(), self.scope_orders.last()) {
            for name in order.iter().rev() {
                scope.remove(name);
            }
        }
        self.scopes.pop();
        self.scope_orders.pop();
    }

    fn evaluate_binary(
        &self,
        operator: BinaryOperator,
        left: Value,
        right: Value,
        span: Span,
    ) -> RuntimeResult<Value> {
        let invalid = || self.error(format!("invalid operands for `{operator:?}`"), span);
        match operator {
            BinaryOperator::Add => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(a), Value::UInt(b)) => a
                    .checked_add(b)
                    .map(Value::UInt)
                    .ok_or_else(|| self.error("uint overflow in addition", span)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a + b)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_add(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in addition", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Subtract => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(a), Value::UInt(b)) => a
                    .checked_sub(b)
                    .map(Value::UInt)
                    .ok_or_else(|| self.error("uint overflow in subtraction", span)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a - b)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_sub(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in subtraction", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Multiply => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(a), Value::UInt(b)) => a
                    .checked_mul(b)
                    .map(Value::UInt)
                    .ok_or_else(|| self.error("uint overflow in multiplication", span)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a * b)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_mul(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in multiplication", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Divide => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(_), Value::UInt(0)) => Err(self.error("division by zero", span)),
                (Value::UInt(a), Value::UInt(b)) => Ok(Value::UInt(a / b)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a / b)),
                (Value::Int(_), Value::Int(0)) => Err(self.error("division by zero", span)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_div(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in division", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Remainder => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(_), Value::UInt(0)) => Err(self.error("remainder by zero", span)),
                (Value::UInt(a), Value::UInt(b)) => Ok(Value::UInt(a % b)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a % b)),
                (Value::Int(_), Value::Int(0)) => Err(self.error("remainder by zero", span)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_rem(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in remainder", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Equal => Ok(Value::Bool(left == right)),
            BinaryOperator::NotEqual => Ok(Value::Bool(left != right)),
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => compare(left, right, operator, span, self),
            BinaryOperator::And | BinaryOperator::Or => {
                Err(self.error("logical operator was not short-circuited", span))
            }
        }
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn assign(&mut self, name: &str, value: Value) -> Option<()> {
        if let Some(Value::CaptureReference(place, mutable)) = self.lookup(name) {
            if !mutable {
                return None;
            }
            return self.write_place(&place, value);
        }
        let scope = self
            .scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))?;
        scope.insert(name.into(), value);
        Some(())
    }

    fn index_value(&mut self, program: &Program, expression: &Expr) -> RuntimeResult<usize> {
        match self.evaluate(program, expression)? {
            Value::Int(value) if value >= 0 => Ok(value as usize),
            Value::UInt(value) => Ok(value as usize),
            Value::Signed(value, _) if value >= 0 => Ok(value as usize),
            Value::Unsigned(value, _) => usize::try_from(value)
                .map_err(|_| self.error("index is too large", expression.span)),
            _ => Err(self.error("index must be a non-negative integer", expression.span)),
        }
    }

    fn expression_place(&self, expression: &Expr) -> Option<Place> {
        match &expression.node {
            Expression::Identifier(name) => {
                if let Some(Value::CaptureReference(place, _)) = self.lookup(name) {
                    return Some(place);
                }
                let scope = self
                    .scopes
                    .iter()
                    .rposition(|scope| scope.contains_key(name))?;
                Some(Place {
                    scope,
                    name: name.clone(),
                    fields: vec![],
                })
            }
            Expression::FieldAccess { object, field, .. } => {
                let mut place = if let Expression::Identifier(name) = &object.node {
                    match self.lookup(name) {
                        Some(Value::Reference(place, _)) => place,
                        _ => self.expression_place(object)?,
                    }
                } else {
                    self.expression_place(object)?
                };
                place.fields.push(PlaceSegment::Field(field.clone()));
                Some(place)
            }
            Expression::Dereference(target) => match &target.node {
                Expression::Identifier(name) => match self.lookup(name)? {
                    Value::Reference(place, _) => Some(place),
                    Value::MutexGuard(_) => {
                        let scope = self
                            .scopes
                            .iter()
                            .rposition(|scope| scope.contains_key(name))?;
                        Some(Place {
                            scope,
                            name: name.clone(),
                            fields: vec![PlaceSegment::MutexValue],
                        })
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    }

    fn dynamic_place(
        &mut self,
        program: &Program,
        expression: &Expr,
    ) -> RuntimeResult<Option<Place>> {
        match &expression.node {
            Expression::Index { object, index } => {
                let Some(mut place) = self.dynamic_place(program, object)? else {
                    return Ok(None);
                };
                let index = self.index_value(program, index)?;
                let length = match self.read_place(&place) {
                    Some(Value::Array(values) | Value::Slice(values)) => values.len(),
                    Some(Value::List { values, .. }) => values.len(),
                    _ => return Ok(None),
                };
                if index >= length {
                    return Err(self.error(
                        format!("array index {index} is out of bounds for length {length}"),
                        expression.span,
                    ));
                }
                place.fields.push(PlaceSegment::Index(index));
                Ok(Some(place))
            }
            Expression::Subslice { object, start, end } => {
                let Some(mut place) = self.dynamic_place(program, object)? else {
                    return Ok(None);
                };
                let start = self.index_value(program, start)?;
                let end = self.index_value(program, end)?;
                let source = self.read_place(&place);
                let length = match &source {
                    Some(Value::Array(values) | Value::Slice(values)) => values.len(),
                    Some(Value::List { values, .. }) => values.len(),
                    Some(Value::String(value)) => value.text.len(),
                    _ => return Ok(None),
                };
                let invalid_utf8 = matches!(
                    &source,
                    Some(Value::String(value))
                        if !value.text.is_char_boundary(start) || !value.text.is_char_boundary(end)
                );
                if start > end || end > length || invalid_utf8 {
                    return Err(self.error(
                        format!("subslice range {start}..{end} is out of bounds or not on UTF-8 boundaries for length {length}"),
                        expression.span,
                    ));
                }
                place.fields.push(PlaceSegment::Subslice(start, end));
                Ok(Some(place))
            }
            _ => Ok(self.expression_place(expression)),
        }
    }

    fn read_place(&self, place: &Place) -> Option<Value> {
        read_value_path(
            self.scopes.get(place.scope)?.get(&place.name)?,
            &place.fields,
        )
    }

    fn write_place(&mut self, place: &Place, value: Value) -> Option<()> {
        let root = self.scopes.get_mut(place.scope)?.get_mut(&place.name)?;
        write_value_path(root, &place.fields, value)
    }

    fn error(&self, message: impl Into<String>, span: Span) -> RuntimeFault {
        RuntimeFault::Error(self.diagnostic(message, span))
    }

    fn diagnostic(&self, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::new(DiagnosticKind::Runtime, message, span)
    }
}

fn write_value_path(target: &mut Value, fields: &[PlaceSegment], value: Value) -> Option<()> {
    if fields.is_empty() {
        *target = value;
        return Some(());
    }
    match &fields[0] {
        PlaceSegment::Field(field) => {
            let Value::Struct {
                fields: members, ..
            } = target
            else {
                return None;
            };
            write_value_path(members.get_mut(field)?, &fields[1..], value)
        }
        PlaceSegment::Index(index) => {
            if let Value::Memory(memory) = target {
                if fields.len() != 1 {
                    return None;
                }
                return memory.write(*index, runtime_byte(value)?);
            }
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            write_value_path(values.get_mut(*index)?, &fields[1..], value)
        }
        PlaceSegment::MapValue(index) => {
            let Value::Map { entries, .. } = target else {
                return None;
            };
            write_value_path(&mut entries.get_mut(*index)?.1, &fields[1..], value)
        }
        PlaceSegment::Subslice(start, end) => {
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            if *start > *end || *end > values.len() {
                return None;
            }
            match fields.get(1)? {
                PlaceSegment::Index(index) if *index < end - start => {
                    write_value_path(values.get_mut(start + index)?, &fields[2..], value)
                }
                _ => None,
            }
        }
        PlaceSegment::MutexValue => {
            let Value::MutexGuard(guard) = target else {
                return None;
            };
            if fields.len() == 1 {
                guard.write(value)
            } else {
                let mut inner = guard.read()?;
                write_value_path(&mut inner, &fields[1..], value)?;
                guard.write(inner)
            }
        }
    }
}

fn read_value_path(target: &Value, fields: &[PlaceSegment]) -> Option<Value> {
    if fields.is_empty() {
        return (!matches!(target, Value::Uninitialized)).then(|| target.clone());
    }
    match &fields[0] {
        PlaceSegment::Field(field) => {
            let Value::Struct {
                fields: members, ..
            } = target
            else {
                return None;
            };
            read_value_path(members.get(field)?, &fields[1..])
        }
        PlaceSegment::Index(index) => {
            if let Value::Memory(memory) = target {
                if fields.len() != 1 {
                    return None;
                }
                return memory
                    .read(*index)
                    .map(|value| Value::Unsigned(value.into(), 8));
            }
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            read_value_path(values.get(*index)?, &fields[1..])
        }
        PlaceSegment::MapValue(index) => {
            let Value::Map { entries, .. } = target else {
                return None;
            };
            read_value_path(&entries.get(*index)?.1, &fields[1..])
        }
        PlaceSegment::Subslice(start, end) => {
            if let Value::String(text) = target {
                if *start > *end
                    || *end > text.text.len()
                    || !text.text.is_char_boundary(*start)
                    || !text.text.is_char_boundary(*end)
                {
                    return None;
                }
                let sliced =
                    Value::String(RuntimeString::literal(text.text[*start..*end].to_owned()));
                return read_value_path(&sliced, &fields[1..]);
            }
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            if *start > *end || *end > values.len() {
                return None;
            }
            if fields.len() == 1 {
                return Some(Value::Slice(values[*start..*end].to_vec()));
            }
            match &fields[1] {
                PlaceSegment::Index(index) if *index < end - start => {
                    read_value_path(values.get(start + index)?, &fields[2..])
                }
                _ => None,
            }
        }
        PlaceSegment::MutexValue => {
            let Value::MutexGuard(guard) = target else {
                return None;
            };
            read_value_path(&guard.read()?, &fields[1..])
        }
    }
}

impl RuntimeFault {
    fn into_diagnostic(self) -> Diagnostic {
        match self {
            Self::Error(diagnostic) => diagnostic,
            Self::Propagate(_) => Diagnostic::new(
                DiagnosticKind::Runtime,
                "error propagation escaped the program entry point",
                Span::point(1, 1),
            ),
        }
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

fn compare(
    left: Value,
    right: Value,
    operator: BinaryOperator,
    span: Span,
    interpreter: &Interpreter,
) -> RuntimeResult<Value> {
    let signed = |a: i128, b: i128| match operator {
        BinaryOperator::Less => a < b,
        BinaryOperator::LessEqual => a <= b,
        BinaryOperator::Greater => a > b,
        BinaryOperator::GreaterEqual => a >= b,
        _ => unreachable!(),
    };
    let unsigned = |a: u128, b: u128| match operator {
        BinaryOperator::Less => a < b,
        BinaryOperator::LessEqual => a <= b,
        BinaryOperator::Greater => a > b,
        BinaryOperator::GreaterEqual => a >= b,
        _ => unreachable!(),
    };
    let float = |a: f64, b: f64| match operator {
        BinaryOperator::Less => a < b,
        BinaryOperator::LessEqual => a <= b,
        BinaryOperator::Greater => a > b,
        BinaryOperator::GreaterEqual => a >= b,
        _ => unreachable!(),
    };
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(signed(a as i128, b as i128))),
        (Value::UInt(a), Value::UInt(b)) => Ok(Value::Bool(unsigned(a as u128, b as u128))),
        (Value::Signed(a, w), Value::Signed(b, v)) if w == v => Ok(Value::Bool(signed(a, b))),
        (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => Ok(Value::Bool(unsigned(a, b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(float(a, b))),
        (Value::Float32(a), Value::Float32(b)) => Ok(Value::Bool(float(a as f64, b as f64))),
        _ => Err(interpreter.error("invalid comparison operands", span)),
    }
}

fn checked_signed(
    a: i128,
    b: i128,
    width: u16,
    operator: BinaryOperator,
    span: Span,
    interpreter: &Interpreter,
) -> RuntimeResult<Value> {
    if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder) && b == 0 {
        return Err(interpreter.error("division by zero", span));
    }
    let value = match operator {
        BinaryOperator::Add => a.checked_add(b),
        BinaryOperator::Subtract => a.checked_sub(b),
        BinaryOperator::Multiply => a.checked_mul(b),
        BinaryOperator::Divide => a.checked_div(b),
        BinaryOperator::Remainder => a.checked_rem(b),
        _ => None,
    };
    let value = value.filter(|value| {
        width == 128 || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1).contains(value)
    });
    value
        .map(|value| Value::Signed(value, width))
        .ok_or_else(|| interpreter.error(format!("i{width} overflow in arithmetic"), span))
}

fn checked_unsigned(
    a: u128,
    b: u128,
    width: u16,
    operator: BinaryOperator,
    span: Span,
    interpreter: &Interpreter,
) -> RuntimeResult<Value> {
    if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder) && b == 0 {
        return Err(interpreter.error("division by zero", span));
    }
    let value = match operator {
        BinaryOperator::Add => a.checked_add(b),
        BinaryOperator::Subtract => a.checked_sub(b),
        BinaryOperator::Multiply => a.checked_mul(b),
        BinaryOperator::Divide => a.checked_div(b),
        BinaryOperator::Remainder => a.checked_rem(b),
        _ => None,
    };
    let value = value.filter(|value| width == 128 || *value < (1_u128 << width));
    value
        .map(|value| Value::Unsigned(value, width))
        .ok_or_else(|| interpreter.error(format!("u{width} overflow in arithmetic"), span))
}

fn value_type_name(value: &Value) -> &str {
    match value {
        Value::Int(_) => "int",
        Value::UInt(_) => "uint",
        Value::Signed(_, 8) => "i8",
        Value::Signed(_, 16) => "i16",
        Value::Signed(_, 32) => "i32",
        Value::Signed(_, 64) => "i64",
        Value::Signed(_, 128) => "i128",
        Value::Signed(_, _) => "<signed>",
        Value::Unsigned(_, 8) => "u8",
        Value::Unsigned(_, 16) => "u16",
        Value::Unsigned(_, 32) => "u32",
        Value::Unsigned(_, 64) => "u64",
        Value::Unsigned(_, 128) => "u128",
        Value::Unsigned(_, _) => "<unsigned>",
        Value::Float(_) => "f64",
        Value::Float32(_) => "f32",
        Value::Reference(_, false) | Value::CaptureReference(_, false) => "&",
        Value::Reference(_, true) | Value::CaptureReference(_, true) => "&mut",
        Value::String(_) => "String",
        Value::CString(_) => "CString",
        Value::CStr(_) => "CStr",
        Value::Memory(_) => "Memory",
        Value::Path(_) => "Path",
        Value::Url(_) => "Url",
        Value::Json(_) => "Json",
        Value::IpAddress(_) => "IpAddress",
        Value::SocketAddress(_) => "SocketAddress",
        Value::TcpStream(_) => "TcpStream",
        Value::TlsStream(_) => "TlsStream",
        Value::HttpRequest(_) => "HttpRequest",
        Value::HttpResponse(_) => "HttpResponse",
        Value::TcpListener(_) => "TcpListener",
        Value::UdpSocket(_) => "UdpSocket",
        Value::UdpDatagram(_) => "UdpDatagram",
        Value::Instant(_) => "Instant",
        Value::Duration(_) => "Duration",
        Value::ProcessOutput(_) => "ProcessOutput",
        Value::ProcessCommand(_) => "ProcessCommand",
        Value::ChildProcess(_) => "ChildProcess",
        Value::Database(_) => "Database",
        Value::Thread(_) => "Thread",
        Value::Future(_) => "Future",
        Value::Task(_) => "Task",
        Value::Mutex(_) => "Mutex",
        Value::MutexGuard(_) => "MutexGuard",
        Value::AtomicInt(_) => "AtomicInt",
        Value::Array(_) => "Array",
        Value::Slice(_) => "Slice",
        Value::List { .. } => "List",
        Value::Map { .. } => "Map",
        Value::Set { .. } => "Set",
        Value::Char(_) => "char",
        Value::Bool(_) => "bool",
        Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => type_name,
        Value::Unit => "Unit",
        Value::Function(_) | Value::Closure(_) | Value::Constructor { .. } => "<callable>",
        Value::Uninitialized => "<uninitialized>",
    }
}

fn value_is_copy(program: &Program, value: &Value) -> bool {
    match value {
        Value::Int(_)
        | Value::UInt(_)
        | Value::Signed(_, _)
        | Value::Unsigned(_, _)
        | Value::Float(_)
        | Value::Float32(_)
        | Value::Instant(_)
        | Value::Duration(_)
        | Value::IpAddress(_)
        | Value::Char(_)
        | Value::Bool(_)
        | Value::Reference(_, _)
        | Value::CaptureReference(_, _)
        | Value::CStr(_)
        | Value::Function(_)
        | Value::Constructor { .. }
        | Value::Unit => true,
        Value::Enum {
            type_name, payload, ..
        } if matches!(type_name.as_str(), "Option" | "Result") => {
            payload.iter().all(|value| value_is_copy(program, value))
        }
        Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => {
            program.implementations.iter().any(|implementation| {
                implementation
                    .trait_name
                    .as_ref()
                    .is_some_and(|trait_name| trait_name.name == "Copy")
                    && implementation.target.name == *type_name
            })
        }
        Value::Array(values) => values.iter().all(|value| value_is_copy(program, value)),
        Value::Slice(_) => true,
        Value::List { .. }
        | Value::Map { .. }
        | Value::Set { .. }
        | Value::Thread(_)
        | Value::Future(_)
        | Value::Task(_)
        | Value::Mutex(_)
        | Value::MutexGuard(_)
        | Value::AtomicInt(_)
        | Value::Path(_)
        | Value::ProcessOutput(_)
        | Value::ProcessCommand(_)
        | Value::ChildProcess(_)
        | Value::Database(_)
        | Value::Url(_)
        | Value::Json(_)
        | Value::SocketAddress(_)
        | Value::TcpStream(_)
        | Value::TlsStream(_)
        | Value::HttpRequest(_)
        | Value::HttpResponse(_)
        | Value::TcpListener(_)
        | Value::UdpSocket(_)
        | Value::UdpDatagram(_)
        | Value::String(_)
        | Value::CString(_)
        | Value::Memory(_)
        | Value::Closure(_)
        | Value::Uninitialized => false,
    }
}

fn coerce_value(value: Value, ty: &crate::ast::TypeName) -> Result<Value, String> {
    let signed_width = match ty.name.as_str() {
        "i8" | "CChar" => Some(8),
        "i16" | "CShort" => Some(16),
        "i32" | "CInt" => Some(32),
        "i64" | "CLongLong" => Some(64),
        "i128" => Some(128),
        _ => None,
    };
    let unsigned_width = match ty.name.as_str() {
        "u8" | "CUChar" => Some(8),
        "u16" | "CUShort" => Some(16),
        "u32" | "CUInt" => Some(32),
        "u64" | "CULongLong" => Some(64),
        "u128" => Some(128),
        _ => None,
    };
    if let Some(width) = signed_width {
        let number = match value {
            Value::Int(value) => value as i128,
            Value::Signed(value, _) => value,
            Value::UInt(value) => value as i128,
            Value::Unsigned(value, _) if value <= i128::MAX as u128 => value as i128,
            Value::Float(value)
                if value.is_finite()
                    && value.fract() == 0.0
                    && value >= i128::MIN as f64
                    && value <= i128::MAX as f64 =>
            {
                value as i128
            }
            Value::Float32(value) if value.is_finite() && value.fract() == 0.0 => value as i128,
            _ => return Err(format!("value cannot be represented as i{width}")),
        };
        let fits = width == 128
            || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1).contains(&number);
        return fits
            .then_some(Value::Signed(number, width))
            .ok_or_else(|| format!("value {number} is outside i{width} range"));
    }
    if let Some(width) = unsigned_width {
        let number = match value {
            Value::Int(value) if value >= 0 => value as u128,
            Value::UInt(value) => value as u128,
            Value::Unsigned(value, _) => value,
            Value::Signed(value, _) if value >= 0 => value as u128,
            Value::Float(value)
                if value.is_finite()
                    && value.fract() == 0.0
                    && value >= 0.0
                    && value <= u128::MAX as f64 =>
            {
                value as u128
            }
            Value::Float32(value) if value.is_finite() && value.fract() == 0.0 && value >= 0.0 => {
                value as u128
            }
            _ => return Err(format!("value cannot be represented as u{width}")),
        };
        let fits = width == 128 || number < (1_u128 << width);
        return fits
            .then_some(Value::Unsigned(number, width))
            .ok_or_else(|| format!("value {number} is outside u{width} range"));
    }
    match (ty.name.as_str(), value) {
        ("uint", Value::Int(value)) if value >= 0 => Ok(Value::UInt(value as u64)),
        ("int", Value::UInt(value)) if value <= i64::MAX as u64 => Ok(Value::Int(value as i64)),
        ("f32" | "CFloat", value) => numeric_as_f64(&value)
            .map(|number| Value::Float32(number as f32))
            .ok_or_else(|| "value is not numeric".into()),
        ("f64" | "CDouble", value) => numeric_as_f64(&value)
            .map(Value::Float)
            .ok_or_else(|| "value is not numeric".into()),
        (_, value) => Ok(value),
    }
}

fn numeric_as_f64(value: &Value) -> Option<f64> {
    Some(match value {
        Value::Int(v) => *v as f64,
        Value::UInt(v) => *v as f64,
        Value::Signed(v, _) => *v as f64,
        Value::Unsigned(v, _) => *v as f64,
        Value::Float(v) => *v,
        Value::Float32(v) => *v as f64,
        _ => return None,
    })
}

fn runtime_byte(value: Value) -> Option<u8> {
    match value {
        Value::Unsigned(value, 8) => u8::try_from(value).ok(),
        Value::UInt(value) => u8::try_from(value).ok(),
        Value::Int(value) => u8::try_from(value).ok(),
        _ => None,
    }
}

fn coerce_like(value: Value, target: &Value) -> Result<Value, String> {
    let name = value_type_name(target);
    coerce_value(
        value,
        &crate::ast::TypeName {
            name: name.into(),
            arguments: vec![],
            qualifier: crate::ast::TypeQualifier::Owned,
            span: Span::point(1, 1),
        },
    )
}

fn coerce_numeric_pair(left: Value, right: Value) -> Result<(Value, Value), String> {
    match (&left, &right) {
        (Value::Signed(_, a), Value::Signed(_, b)) if a < b => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Signed(_, a), Value::Signed(_, b)) if b < a => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Unsigned(_, a), Value::Unsigned(_, b)) if a < b => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Unsigned(_, a), Value::Unsigned(_, b)) if b < a => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Unsigned(_, unsigned), Value::Signed(_, signed)) if signed > unsigned => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Signed(_, signed), Value::Unsigned(_, unsigned)) if signed > unsigned => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Int(_), Value::Signed(_, width)) if *width >= 128 => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Signed(_, width), Value::Int(_)) if *width >= 128 => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::UInt(_), Value::Unsigned(_, width)) if *width >= 128 => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Unsigned(_, width), Value::UInt(_)) if *width >= 128 => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Float32(_), Value::Float(_)) => Ok((coerce_like(left, &right)?, right)),
        (Value::Float(_), Value::Float32(_)) => Ok((left.clone(), coerce_like(right, &left)?)),
        (
            Value::Signed(_, _) | Value::Unsigned(_, _) | Value::UInt(_) | Value::Float32(_),
            Value::Int(_) | Value::Float(_),
        ) => Ok((left.clone(), coerce_like(right, &left)?)),
        _ => Ok((left, right)),
    }
}

fn is_numeric_name(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "uint"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "f32"
            | "f64"
    )
}

fn integer_method(method: &str, left: Value, right: Value) -> Result<Value, String> {
    let wrapping = method.starts_with("wrapping_");
    let operation = method.rsplit('_').next().unwrap();
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match (wrapping, operation) {
            (true, "add") => a.wrapping_add(b),
            (true, "sub") => a.wrapping_sub(b),
            (true, "mul") => a.wrapping_mul(b),
            (false, "add") => a.saturating_add(b),
            (false, "sub") => a.saturating_sub(b),
            (false, "mul") => a.saturating_mul(b),
            _ => unreachable!(),
        })),
        (Value::UInt(a), Value::UInt(b)) => Ok(Value::UInt(match (wrapping, operation) {
            (true, "add") => a.wrapping_add(b),
            (true, "sub") => a.wrapping_sub(b),
            (true, "mul") => a.wrapping_mul(b),
            (false, "add") => a.saturating_add(b),
            (false, "sub") => a.saturating_sub(b),
            (false, "mul") => a.saturating_mul(b),
            _ => unreachable!(),
        })),
        (Value::Signed(a, width), Value::Signed(b, other)) if width == other => {
            let (min, max) = if width == 128 {
                (i128::MIN, i128::MAX)
            } else {
                (-(1_i128 << (width - 1)), (1_i128 << (width - 1)) - 1)
            };
            let value = if wrapping {
                let raw = match operation {
                    "add" => a.wrapping_add(b),
                    "sub" => a.wrapping_sub(b),
                    "mul" => a.wrapping_mul(b),
                    _ => unreachable!(),
                };
                if width == 128 {
                    raw
                } else {
                    let modulus = 1_i128 << width;
                    let masked = raw & (modulus - 1);
                    if masked > max {
                        masked - modulus
                    } else {
                        masked
                    }
                }
            } else {
                match operation {
                    "add" => a.saturating_add(b),
                    "sub" => a.saturating_sub(b),
                    "mul" => a.saturating_mul(b),
                    _ => unreachable!(),
                }
                .clamp(min, max)
            };
            Ok(Value::Signed(value, width))
        }
        (Value::Unsigned(a, width), Value::Unsigned(b, other)) if width == other => {
            let max = if width == 128 {
                u128::MAX
            } else {
                (1_u128 << width) - 1
            };
            let value = if wrapping {
                let raw = match operation {
                    "add" => a.wrapping_add(b),
                    "sub" => a.wrapping_sub(b),
                    "mul" => a.wrapping_mul(b),
                    _ => unreachable!(),
                };
                raw & max
            } else {
                match operation {
                    "add" => a.saturating_add(b),
                    "sub" => a.saturating_sub(b),
                    "mul" => a.saturating_mul(b),
                    _ => unreachable!(),
                }
                .min(max)
            };
            Ok(Value::Unsigned(value, width))
        }
        _ => Err(format!("invalid operands for `{method}`")),
    }
}

fn find_variants<'a>(
    program: &'a Program,
    name: &str,
) -> Vec<(&'a EnumDeclaration, &'a VariantDeclaration)> {
    program
        .enums
        .iter()
        .flat_map(|owner| {
            owner
                .variants
                .iter()
                .filter(move |variant| variant.name == name)
                .map(move |variant| (owner, variant))
        })
        .collect()
}

fn find_qualified_variant<'a>(
    program: &'a Program,
    type_name: &str,
    variant_name: &str,
) -> Option<(&'a EnumDeclaration, &'a VariantDeclaration)> {
    let owner = program.enums.iter().find(|owner| owner.name == type_name)?;
    let variant = owner
        .variants
        .iter()
        .find(|variant| variant.name == variant_name)?;
    Some((owner, variant))
}

fn pattern_matches(
    pattern: &Pattern,
    value: &Value,
    bindings: &mut HashMap<String, Value>,
) -> bool {
    match pattern {
        Pattern::Wildcard => true,
        Pattern::Binding(name) => {
            bindings.insert(name.clone(), value.clone());
            true
        }
        Pattern::Integer(pattern) => match value {
            Value::Int(value) => *value >= 0 && *value as u128 == *pattern,
            Value::UInt(value) => *value as u128 == *pattern,
            Value::Signed(value, _) => *value >= 0 && *value as u128 == *pattern,
            Value::Unsigned(value, _) => *value == *pattern,
            _ => false,
        },
        Pattern::String(pattern) => matches!(value, Value::String(value) if value.text == *pattern),
        Pattern::Character(pattern) => matches!(value, Value::Char(value) if value == pattern),
        Pattern::Bool(pattern) => matches!(value, Value::Bool(value) if value == pattern),
        Pattern::Variant {
            type_name,
            variant,
            arguments,
        } => {
            let Value::Enum {
                type_name: value_type,
                variant: value_variant,
                payload,
            } = value
            else {
                return false;
            };
            if value_variant != variant
                || type_name
                    .as_ref()
                    .is_some_and(|pattern_type| pattern_type != value_type)
                || arguments.len() != payload.len()
            {
                return false;
            }
            arguments
                .iter()
                .zip(payload)
                .all(|(pattern, value)| pattern_matches(&pattern.node, value, bindings))
        }
    }
}

fn display_value(value: Value) -> String {
    match value {
        Value::Int(value) => value.to_string(),
        Value::UInt(value) => value.to_string(),
        Value::Signed(value, _) => value.to_string(),
        Value::Unsigned(value, _) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Float32(value) => value.to_string(),
        Value::Reference(_, false) | Value::CaptureReference(_, false) => {
            "<shared reference>".into()
        }
        Value::Reference(_, true) | Value::CaptureReference(_, true) => {
            "<mutable reference>".into()
        }
        Value::String(value) => value.text,
        Value::CString(value) | Value::CStr(value) => value.text(),
        Value::Memory(_) => "<Memory>".into(),
        Value::Path(value) => value.to_string_lossy().into_owned(),
        Value::Url(value) => value.text,
        Value::Json(value) => value.text,
        Value::IpAddress(value) => value.0.to_string(),
        Value::SocketAddress(value) => format!("{}:{}", value.host, value.port),
        Value::TcpStream(_) => "<TcpStream>".into(),
        Value::TlsStream(_) => "<TlsStream>".into(),
        Value::HttpRequest(value) => format!("<HttpRequest:{}>", value.method),
        Value::HttpResponse(value) => {
            format!(
                "<HttpResponse:{} {} bytes>",
                value.0.status,
                value.0.body.len()
            )
        }
        Value::TcpListener(_) => "<TcpListener>".into(),
        Value::UdpSocket(_) => "<UdpSocket>".into(),
        Value::UdpDatagram(value) => format!("<UdpDatagram:{} bytes>", value.bytes.len()),
        Value::Instant(_) => "<Instant>".into(),
        Value::Duration(value) => format!("{}ns", value.as_nanos()),
        Value::ProcessOutput(value) => format!("<ProcessOutput:{}>", value.status),
        Value::ProcessCommand(_) => "<ProcessCommand>".into(),
        Value::ChildProcess(_) => "<ChildProcess>".into(),
        Value::Database(_) => "<Database>".into(),
        Value::Thread(_) => "<Thread>".into(),
        Value::Future(_) => "<Future>".into(),
        Value::Task(_) => "<Task>".into(),
        Value::Mutex(_) => "<Mutex>".into(),
        Value::MutexGuard(_) => "<MutexGuard>".into(),
        Value::AtomicInt(_) => "<AtomicInt>".into(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Slice(values) => format!(
            "[{}]",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::List { values, .. } => format!(
            "[{}]",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Map { entries, .. } => format!(
            "{{{}}}",
            entries
                .into_iter()
                .map(|(key, value)| format!("{}: {}", display_value(key), display_value(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Set { values, .. } => format!(
            "{{{}}}",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Char(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Function(name) => format!("<fn {name}>"),
        Value::Closure(_) => "<closure>".into(),
        Value::Constructor { type_name, variant } => format!("<{type_name}.{variant}>"),
        Value::Struct { type_name, .. } => format!("{type_name} {{ .. }}"),
        Value::Enum {
            type_name,
            variant,
            payload,
        } => {
            if payload.is_empty() {
                format!("{type_name}.{variant}")
            } else {
                let values = payload
                    .into_iter()
                    .map(display_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{type_name}.{variant}({values})")
            }
        }
        Value::Unit => "()".into(),
        Value::Uninitialized => "<uninitialized>".into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::run_source;

    #[test]
    fn executes_functions_control_flow_and_assignments() {
        let output = run_source(
            "fn sum_to(n: int) -> int { var total = 0 var i = 0 while i <= n { total += i i += 1 } return total } fn main() { print(sum_to(10)) }",
        )
        .expect("program should run");
        assert_eq!(output, ["55"]);
    }

    #[test]
    fn executes_exclusive_and_inclusive_for_ranges() {
        let output = run_source(
            "fn main() { var total = 0 for i in 0..5 { total += i } print(total) var count = 0 for i in 0..=5 { count += 1 } print(count) }",
        )
        .expect("for ranges should run");
        assert_eq!(output, ["10", "6"]);
    }

    #[test]
    fn return_break_and_continue_propagate_correctly() {
        let output = run_source(
            "fn first() -> int { var i = 0 loop { i += 1 if i == 2 { continue } if i == 4 { return i } } } fn main() { print(first()) }",
        )
        .expect("program should run");
        assert_eq!(output, ["4"]);
    }

    #[test]
    fn logical_operators_short_circuit() {
        let output =
            run_source("fn main() { print(false && (1 / 0 == 0)) print(true || (1 / 0 == 0)) }")
                .expect("short-circuited expressions must not divide by zero");
        assert_eq!(output, ["false", "true"]);
    }

    #[test]
    fn executes_recursive_functions_with_isolated_call_frames() {
        let output = run_source(
            "fn factorial(n: int) -> int { if n <= 1 { return 1 } else { return n * factorial(n - 1) } } fn main() { print(factorial(6)) }",
        )
        .expect("recursive program should run");
        assert_eq!(output, ["720"]);
    }

    #[test]
    fn reports_checked_integer_failures() {
        let error = run_source("fn main() { print(9223372036854775807 + 1) }")
            .expect_err("overflow should fail");
        assert!(error.message.contains("overflow"));
        let error = run_source("fn main() { print(1 / 0) }").expect_err("division should fail");
        assert!(error.message.contains("zero"));
    }

    #[test]
    fn limits_recursive_runtime_calls() {
        let error =
            run_source("fn recurse() -> int { return recurse() } fn main() { print(recurse()) }")
                .expect_err("unbounded recursion should be stopped");
        assert!(error.message.contains("call depth"));
    }
}
