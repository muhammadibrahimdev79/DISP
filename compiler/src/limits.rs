//! Canonical, host-independent safety limits for the DISP compiler and runtime.
//!
//! Keep policy values here. Enforcement remains close to the resource it meters,
//! while native code receives the same runtime defaults through [`native_prelude`].

// Compiler input and work budgets.
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PROJECT_MODULES: usize = 1_024;
pub const MAX_PROJECT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_MODULE_DEPTH: usize = 128;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_PACKAGES: usize = 512;
pub const MAX_DEPENDENCY_DEPTH: usize = 128;
pub const MAX_PACKAGE_FILES: usize = 16_384;
pub const MAX_PACKAGE_SOURCE_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_LOCKFILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_EXPANSION_DEPTH: usize = 64;
pub const MAX_GENERATED_NODES: usize = 65_536;
pub const MAX_REPEAT_COUNT: usize = 4_096;
pub const MAX_CONST_STEPS: usize = 100_000;
pub const MAX_CONST_DEPTH: usize = 128;
pub const MAX_CONST_VALUE_NODES: usize = 65_536;
pub const MAX_CONST_STRING_BYTES: usize = 1024 * 1024;
pub const MAX_EXPRESSION_DEPTH: usize = 32;
pub const MAX_OPERATOR_CHAIN: usize = 256;
pub const MAX_CALL_CHAIN: usize = 256;
pub const MAX_MONOMORPHIZATIONS: usize = 16_384;
pub const MAX_GENERATED_C_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_GENERATED_HEADER_BYTES: usize = 16 * 1024 * 1024;

// Runtime defaults. A deployment may lower these with the validated DISP_MAX_*
// environment controls documented in docs/language/RESOURCE_LIMITS.md.
pub const DEFAULT_RUNTIME_MEMORY_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_RUNTIME_STEPS: u64 = 100_000_000;
pub const DEFAULT_RUNTIME_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_RUNTIME_CALL_DEPTH: usize = 32;
pub const DEFAULT_RUNTIME_TASKS: usize = 4_096;
pub const DEFAULT_RUNTIME_THREADS: usize = 256;
pub const DEFAULT_RUNTIME_PROCESS_STARTS: usize = 256;
pub const DEFAULT_RUNTIME_HANDLES: usize = 4_096;
pub const DEFAULT_RUNTIME_FILE_WRITE_BYTES: usize = 64 * 1024 * 1024;

// OS-enforced child-tree sandbox defaults.
pub const DEFAULT_CHILD_MEMORY_BYTES: usize = 512 * 1024 * 1024;
pub const DEFAULT_CHILD_CPU_MILLIS: usize = 60_000;
pub const DEFAULT_CHILD_PROCESSES: usize = 64;
pub const DEFAULT_CHILD_WALL_MILLIS: usize = 24 * 60 * 60 * 1_000;
pub const DEFAULT_TOOL_MEMORY_BYTES: usize = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_TOOL_CPU_MILLIS: usize = 5 * 60 * 1_000;
pub const DEFAULT_TOOL_PROCESSES: usize = 256;
pub const DEFAULT_TOOL_WALL_MILLIS: usize = 10 * 60 * 1_000;
pub const DEFAULT_TOOL_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_COMPONENT_MEMORY_BYTES: usize = 256 * 1024 * 1024;
pub const DEFAULT_COMPONENT_CPU_MILLIS: usize = 10_000;
pub const DEFAULT_COMPONENT_PROCESSES: usize = 8;
pub const DEFAULT_COMPONENT_WALL_MILLIS: usize = 30_000;
pub const DEFAULT_COMPONENT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

// Protocol, storage, and child-process boundaries shared by both engines.
pub const URL_BYTES: usize = 8_192;
pub const JSON_BYTES: usize = 16 * 1024 * 1024;
pub const JSON_DEPTH: usize = 128;
pub const JSON_OBJECT_KEYS: usize = 4_096;
pub const HTTP_HEADER_BYTES: usize = 64 * 1024;
pub const HTTP_BODY_BYTES: usize = 16 * 1024 * 1024;
pub const HTTP_REDIRECTS: usize = 10;
pub const HTTP_HEADERS: usize = 100;
pub const DATABASE_SQL_BYTES: usize = 1024 * 1024;
pub const DATABASE_ROWS: usize = 100_000;
pub const DATABASE_COLUMNS: usize = 4_096;
pub const PROCESS_ARGUMENTS: usize = 4_096;
pub const PROCESS_ARGUMENT_BYTES: usize = 1024 * 1024;
pub const PROCESS_STREAM_BYTES: usize = 16 * 1024 * 1024;
pub const COMPONENT_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
pub const TCP_READ_BYTES: usize = 16 * 1024 * 1024;
pub const UDP_RECEIVE_BYTES: usize = 65_535;
pub const UDP_PAYLOAD_BYTES: usize = 65_507;

pub const TYPE_CHECKER_STACK_BYTES: usize = 16 * 1024 * 1024;
pub const INTERPRETER_STACK_BYTES: usize = 32 * 1024 * 1024;

/// Emits the canonical runtime policy before the embedded C allocator/runtime.
pub fn native_prelude() -> String {
    format!(
        "#ifndef _WIN32\n\
#ifndef _GNU_SOURCE\n\
#define _GNU_SOURCE\n\
#endif\n\
#endif\n\
#define DISP_DEFAULT_MAX_MEMORY_BYTES {DEFAULT_RUNTIME_MEMORY_BYTES}ULL\n\
#define DISP_DEFAULT_MAX_STEPS {DEFAULT_RUNTIME_STEPS}ULL\n\
#define DISP_DEFAULT_MAX_OUTPUT_BYTES {DEFAULT_RUNTIME_OUTPUT_BYTES}ULL\n\
#define DISP_DEFAULT_MAX_CALL_DEPTH {DEFAULT_RUNTIME_CALL_DEPTH}ULL\n\
#define DISP_DEFAULT_MAX_TASKS {DEFAULT_RUNTIME_TASKS}ULL\n\
#define DISP_DEFAULT_MAX_THREADS {DEFAULT_RUNTIME_THREADS}ULL\n\
#define DISP_DEFAULT_MAX_PROCESS_STARTS {DEFAULT_RUNTIME_PROCESS_STARTS}ULL\n\
#define DISP_DEFAULT_MAX_HANDLES {DEFAULT_RUNTIME_HANDLES}ULL\n\
#define DISP_DEFAULT_MAX_FILE_WRITE_BYTES {DEFAULT_RUNTIME_FILE_WRITE_BYTES}ULL\n\
#define DISP_DEFAULT_CHILD_MEMORY_BYTES {DEFAULT_CHILD_MEMORY_BYTES}ULL\n\
#define DISP_DEFAULT_CHILD_CPU_MILLIS {DEFAULT_CHILD_CPU_MILLIS}ULL\n\
#define DISP_DEFAULT_CHILD_PROCESSES {DEFAULT_CHILD_PROCESSES}ULL\n\
#define DISP_DEFAULT_CHILD_WALL_MILLIS {DEFAULT_CHILD_WALL_MILLIS}ULL\n\
#define DISP_URL_LIMIT {URL_BYTES}ULL\n\
#define DISP_JSON_LIMIT {JSON_BYTES}ULL\n\
#define DISP_JSON_DEPTH_LIMIT {JSON_DEPTH}ULL\n\
#define DISP_JSON_KEY_LIMIT {JSON_OBJECT_KEYS}ULL\n\
#define DISP_HTTP_HEADER_LIMIT {HTTP_HEADER_BYTES}ULL\n\
#define DISP_HTTP_BODY_LIMIT {HTTP_BODY_BYTES}ULL\n\
#define DISP_HTTP_REDIRECT_LIMIT {HTTP_REDIRECTS}ULL\n\
#define DISP_HTTP_HEADER_COUNT_LIMIT {HTTP_HEADERS}ULL\n\
#define DISP_DATABASE_SQL_LIMIT {DATABASE_SQL_BYTES}ULL\n\
#define DISP_DATABASE_ROW_LIMIT {DATABASE_ROWS}ULL\n\
#define DISP_DATABASE_COLUMN_LIMIT {DATABASE_COLUMNS}ULL\n\
#define DISP_PROCESS_MAX_ARGUMENTS {PROCESS_ARGUMENTS}ULL\n\
#define DISP_PROCESS_MAX_ARGUMENT_BYTES {PROCESS_ARGUMENT_BYTES}ULL\n\
#define DISP_PROCESS_MAX_CAPTURE {PROCESS_STREAM_BYTES}ULL\n\
#define DISP_TCP_READ_LIMIT {TCP_READ_BYTES}ULL\n\
#define DISP_UDP_RECEIVE_LIMIT {UDP_RECEIVE_BYTES}ULL\n\
#define DISP_UDP_PAYLOAD_LIMIT {UDP_PAYLOAD_BYTES}ULL\n"
    )
}
