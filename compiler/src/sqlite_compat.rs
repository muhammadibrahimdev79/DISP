//! Dynamically loaded legacy SQLite compatibility boundary.
//!
//! The bootstrap compiler must start and run native DISP Data programs without an SQLite
//! installation. SQLite is resolved only when a source program explicitly constructs `Database`.

use std::{ffi::c_void, io, sync::OnceLock};

#[repr(C)]
pub(crate) struct Sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
pub(crate) struct SqliteStatement {
    _private: [u8; 0],
}

type SqliteCallback = unsafe extern "C" fn(
    context: *mut c_void,
    columns: std::ffi::c_int,
    values: *mut *mut std::ffi::c_char,
    names: *mut *mut std::ffi::c_char,
) -> std::ffi::c_int;

type Open = unsafe extern "C" fn(
    *const std::ffi::c_char,
    *mut *mut Sqlite3,
    std::ffi::c_int,
    *const std::ffi::c_char,
) -> std::ffi::c_int;
type Close = unsafe extern "C" fn(*mut Sqlite3) -> std::ffi::c_int;
type ErrorMessage = unsafe extern "C" fn(*mut Sqlite3) -> *const std::ffi::c_char;
type BusyTimeout = unsafe extern "C" fn(*mut Sqlite3, std::ffi::c_int) -> std::ffi::c_int;
type Prepare = unsafe extern "C" fn(
    *mut Sqlite3,
    *const std::ffi::c_char,
    std::ffi::c_int,
    *mut *mut SqliteStatement,
    *mut *const std::ffi::c_char,
) -> std::ffi::c_int;
type Finalize = unsafe extern "C" fn(*mut SqliteStatement) -> std::ffi::c_int;
type Step = unsafe extern "C" fn(*mut SqliteStatement) -> std::ffi::c_int;
type BindParameterCount = unsafe extern "C" fn(*mut SqliteStatement) -> std::ffi::c_int;
type BindNull = unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int) -> std::ffi::c_int;
type BindInt64 =
    unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int, i64) -> std::ffi::c_int;
type BindDouble =
    unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int, f64) -> std::ffi::c_int;
type BindText = unsafe extern "C" fn(
    *mut SqliteStatement,
    std::ffi::c_int,
    *const std::ffi::c_char,
    std::ffi::c_int,
    Option<unsafe extern "C" fn(*mut c_void)>,
) -> std::ffi::c_int;
type ColumnCount = unsafe extern "C" fn(*mut SqliteStatement) -> std::ffi::c_int;
type ColumnName =
    unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int) -> *const std::ffi::c_char;
type ColumnType = unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int) -> std::ffi::c_int;
type ColumnInt64 = unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int) -> i64;
type ColumnDouble = unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int) -> f64;
type ColumnText = unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int) -> *const u8;
type ColumnBytes = unsafe extern "C" fn(*mut SqliteStatement, std::ffi::c_int) -> std::ffi::c_int;
type Changes = unsafe extern "C" fn(*mut Sqlite3) -> std::ffi::c_int;
type LastInsertRowId = unsafe extern "C" fn(*mut Sqlite3) -> i64;
type GetAutocommit = unsafe extern "C" fn(*mut Sqlite3) -> std::ffi::c_int;
type Exec = unsafe extern "C" fn(
    *mut Sqlite3,
    *const std::ffi::c_char,
    Option<SqliteCallback>,
    *mut c_void,
    *mut *mut std::ffi::c_char,
) -> std::ffi::c_int;

struct SqliteApi {
    // The library is intentionally held for the process lifetime so function pointers stay valid.
    _library: usize,
    sqlite3_open_v2: Open,
    sqlite3_close_v2: Close,
    sqlite3_errmsg: ErrorMessage,
    sqlite3_busy_timeout: BusyTimeout,
    sqlite3_prepare_v2: Prepare,
    sqlite3_finalize: Finalize,
    sqlite3_step: Step,
    sqlite3_bind_parameter_count: BindParameterCount,
    sqlite3_bind_null: BindNull,
    sqlite3_bind_int64: BindInt64,
    sqlite3_bind_double: BindDouble,
    sqlite3_bind_text: BindText,
    sqlite3_column_count: ColumnCount,
    sqlite3_column_name: ColumnName,
    sqlite3_column_type: ColumnType,
    sqlite3_column_int64: ColumnInt64,
    sqlite3_column_double: ColumnDouble,
    sqlite3_column_text: ColumnText,
    sqlite3_column_bytes: ColumnBytes,
    sqlite3_changes: Changes,
    sqlite3_last_insert_rowid: LastInsertRowId,
    sqlite3_get_autocommit: GetAutocommit,
    sqlite3_exec: Exec,
}

static SQLITE_API: OnceLock<Result<SqliteApi, String>> = OnceLock::new();

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

#[cfg(windows)]
fn load_library() -> Result<usize, String> {
    for candidate in ["winsqlite3.dll", "sqlite3.dll"] {
        let wide = candidate
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: `wide` is NUL-terminated and remains live for the call.
        let library = unsafe { LoadLibraryW(wide.as_ptr()) };
        if !library.is_null() {
            return Ok(library as usize);
        }
    }
    Err("SQLite compatibility connector is unavailable; install SQLite or use native DISP DataStore"
        .to_owned())
}

#[cfg(not(windows))]
fn load_library() -> Result<usize, String> {
    #[cfg(target_os = "macos")]
    let candidates = [c"libsqlite3.dylib".as_ptr()];
    #[cfg(not(target_os = "macos"))]
    let candidates = [c"libsqlite3.so.0".as_ptr(), c"libsqlite3.so".as_ptr()];
    for candidate in candidates {
        // SAFETY: each candidate is a static NUL-terminated C string.
        let library = unsafe { libc::dlopen(candidate, libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if !library.is_null() {
            return Ok(library as usize);
        }
    }
    Err("SQLite compatibility connector is unavailable; install SQLite or use native DISP DataStore"
        .to_owned())
}

#[cfg(windows)]
unsafe fn load_symbol(library: usize, name: &'static [u8]) -> Result<*mut c_void, String> {
    // SAFETY: the library handle is retained for the process lifetime and name is NUL-terminated.
    let symbol = unsafe { GetProcAddress(library as *mut c_void, name.as_ptr()) };
    if symbol.is_null() {
        Err(format!(
            "SQLite compatibility connector lacks `{}`",
            String::from_utf8_lossy(&name[..name.len().saturating_sub(1)])
        ))
    } else {
        Ok(symbol)
    }
}

#[cfg(not(windows))]
unsafe fn load_symbol(library: usize, name: &'static [u8]) -> Result<*mut c_void, String> {
    // SAFETY: the library handle is retained for the process lifetime and name is NUL-terminated.
    let symbol = unsafe { libc::dlsym(library as *mut c_void, name.as_ptr().cast()) };
    if symbol.is_null() {
        Err(format!(
            "SQLite compatibility connector lacks `{}`",
            String::from_utf8_lossy(&name[..name.len().saturating_sub(1)])
        ))
    } else {
        Ok(symbol)
    }
}

impl SqliteApi {
    fn load() -> Result<Self, String> {
        let library = load_library()?;
        macro_rules! symbol {
            ($name:ident, $ty:ty) => {{
                // SAFETY: SQLite's stable C ABI defines this symbol with exactly `$ty`.
                unsafe {
                    std::mem::transmute::<*mut c_void, $ty>(load_symbol(
                        library,
                        concat!(stringify!($name), "\0").as_bytes(),
                    )?)
                }
            }};
        }
        Ok(Self {
            _library: library,
            sqlite3_open_v2: symbol!(sqlite3_open_v2, Open),
            sqlite3_close_v2: symbol!(sqlite3_close_v2, Close),
            sqlite3_errmsg: symbol!(sqlite3_errmsg, ErrorMessage),
            sqlite3_busy_timeout: symbol!(sqlite3_busy_timeout, BusyTimeout),
            sqlite3_prepare_v2: symbol!(sqlite3_prepare_v2, Prepare),
            sqlite3_finalize: symbol!(sqlite3_finalize, Finalize),
            sqlite3_step: symbol!(sqlite3_step, Step),
            sqlite3_bind_parameter_count: symbol!(sqlite3_bind_parameter_count, BindParameterCount),
            sqlite3_bind_null: symbol!(sqlite3_bind_null, BindNull),
            sqlite3_bind_int64: symbol!(sqlite3_bind_int64, BindInt64),
            sqlite3_bind_double: symbol!(sqlite3_bind_double, BindDouble),
            sqlite3_bind_text: symbol!(sqlite3_bind_text, BindText),
            sqlite3_column_count: symbol!(sqlite3_column_count, ColumnCount),
            sqlite3_column_name: symbol!(sqlite3_column_name, ColumnName),
            sqlite3_column_type: symbol!(sqlite3_column_type, ColumnType),
            sqlite3_column_int64: symbol!(sqlite3_column_int64, ColumnInt64),
            sqlite3_column_double: symbol!(sqlite3_column_double, ColumnDouble),
            sqlite3_column_text: symbol!(sqlite3_column_text, ColumnText),
            sqlite3_column_bytes: symbol!(sqlite3_column_bytes, ColumnBytes),
            sqlite3_changes: symbol!(sqlite3_changes, Changes),
            sqlite3_last_insert_rowid: symbol!(sqlite3_last_insert_rowid, LastInsertRowId),
            sqlite3_get_autocommit: symbol!(sqlite3_get_autocommit, GetAutocommit),
            sqlite3_exec: symbol!(sqlite3_exec, Exec),
        })
    }
}

pub(crate) fn load_sqlite_api() -> io::Result<()> {
    SQLITE_API
        .get_or_init(SqliteApi::load)
        .as_ref()
        .map(|_| ())
        .map_err(|message| io::Error::new(io::ErrorKind::NotFound, message.clone()))
}

fn api() -> &'static SqliteApi {
    SQLITE_API
        .get()
        .and_then(|result| result.as_ref().ok())
        .expect("SQLite API is loaded before a compatibility handle is created")
}

macro_rules! forward {
    ($(fn $name:ident($($argument:ident: $ty:ty),* $(,)?) -> $result:ty;)+) => {$(
        pub(crate) unsafe fn $name($($argument: $ty),*) -> $result {
            // SAFETY: callers uphold SQLite's C ABI contract and `api` retains the library.
            unsafe { (api().$name)($($argument),*) }
        }
    )+};
}

forward! {
    fn sqlite3_open_v2(filename: *const std::ffi::c_char, database: *mut *mut Sqlite3, flags: std::ffi::c_int, vfs: *const std::ffi::c_char) -> std::ffi::c_int;
    fn sqlite3_close_v2(database: *mut Sqlite3) -> std::ffi::c_int;
    fn sqlite3_errmsg(database: *mut Sqlite3) -> *const std::ffi::c_char;
    fn sqlite3_busy_timeout(database: *mut Sqlite3, millis: std::ffi::c_int) -> std::ffi::c_int;
    fn sqlite3_prepare_v2(database: *mut Sqlite3, sql: *const std::ffi::c_char, bytes: std::ffi::c_int, statement: *mut *mut SqliteStatement, tail: *mut *const std::ffi::c_char) -> std::ffi::c_int;
    fn sqlite3_finalize(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_step(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_bind_parameter_count(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_bind_null(statement: *mut SqliteStatement, index: std::ffi::c_int) -> std::ffi::c_int;
    fn sqlite3_bind_int64(statement: *mut SqliteStatement, index: std::ffi::c_int, value: i64) -> std::ffi::c_int;
    fn sqlite3_bind_double(statement: *mut SqliteStatement, index: std::ffi::c_int, value: f64) -> std::ffi::c_int;
    fn sqlite3_bind_text(statement: *mut SqliteStatement, index: std::ffi::c_int, value: *const std::ffi::c_char, bytes: std::ffi::c_int, destructor: Option<unsafe extern "C" fn(*mut c_void)>) -> std::ffi::c_int;
    fn sqlite3_column_count(statement: *mut SqliteStatement) -> std::ffi::c_int;
    fn sqlite3_column_name(statement: *mut SqliteStatement, column: std::ffi::c_int) -> *const std::ffi::c_char;
    fn sqlite3_column_type(statement: *mut SqliteStatement, column: std::ffi::c_int) -> std::ffi::c_int;
    fn sqlite3_column_int64(statement: *mut SqliteStatement, column: std::ffi::c_int) -> i64;
    fn sqlite3_column_double(statement: *mut SqliteStatement, column: std::ffi::c_int) -> f64;
    fn sqlite3_column_text(statement: *mut SqliteStatement, column: std::ffi::c_int) -> *const u8;
    fn sqlite3_column_bytes(statement: *mut SqliteStatement, column: std::ffi::c_int) -> std::ffi::c_int;
    fn sqlite3_changes(database: *mut Sqlite3) -> std::ffi::c_int;
    fn sqlite3_last_insert_rowid(database: *mut Sqlite3) -> i64;
    fn sqlite3_get_autocommit(database: *mut Sqlite3) -> std::ffi::c_int;
    fn sqlite3_exec(database: *mut Sqlite3, sql: *const std::ffi::c_char, callback: Option<SqliteCallback>, context: *mut c_void, error: *mut *mut std::ffi::c_char) -> std::ffi::c_int;
}
