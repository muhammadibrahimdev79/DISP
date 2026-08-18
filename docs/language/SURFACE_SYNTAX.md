# DISP surface syntax

DISP keeps one checked language model while offering two compatible notation levels.
Ordinary code uses inference and safe implicit operations; explicit types, references,
and allocation controls remain available when a program needs them.

## Inferred bindings

The first plain assignment in a scope declares a mutable, statically typed local:

```disp
score = 10
score += 5
```

Its type never changes. `score = true` is a compile-time error. `let`, `var`, `const`,
and type annotations remain supported for code that wants the distinction to be visible.

## Collections and text

```disp
names = List.of("Ari", "Mina")
names.add("Sam")
print(names.count())

scores = Map.of("Ari": 95, "Mina": 98)
scores.set("Sam", 91)

tags = Set.of("safe", "fast", "safe")
tags.add("easy")

title = "Data Intelligence System Page"
word = title.slice(5, 17)
print(word)
```

`slice` creates a checked shared view. `slice_mut` creates an exclusive mutable view.
Both are zero-copy and retain the same ownership and lifetime rules as explicit DISP
references.

## Modules and local packages

One source file is one module. The declaration is optional, but when present it must
match the file path so a module cannot silently impersonate another module:

```disp
module geometry

pub struct Point { x: int, y: int }
fn square(value: int) -> int = value * value
pub fn length_squared(point: Point) -> int = square(point.x) + square(point.y)
```

An entry file imports the public surface with the module path. Selective imports keep
large programs explicit, and aliases resolve same-named APIs without weakening nominal
type identity:

```disp
use geometry
use left.{Token as LeftToken}
use right.{Token as RightToken}
```

`pub use geometry.{Point}` re-exports selected public items. Wildcard module imports
fail on name conflicts instead of guessing. A public signature cannot expose a private
type or trait. Import cycles, path escapes, duplicate module identities, excessive
depth, excessive module counts, and excessive aggregate source size are rejected before
semantic compilation. Interpreter and native runtime diagnostics identify the actual
module and local source position.

The ordinary project layout is:

```text
hello/
|-- DISP.toml
`-- src/
    |-- main.disp
    `-- geometry.disp
```

The implemented manifest is deliberately strict:

```toml
[package]
name = "hello"
version = "0.1.0"
edition = "1"
entry = "src/main.disp"
```

Run `disp new hello` to create this layout. `disp check hello`, `disp build hello`,
`disp run hello`, and `disp interpret hello` accept the directory directly. Package
names are bounded lowercase ASCII identifiers, versions use numeric
`MAJOR.MINOR.PATCH`, and entries must remain inside the project. Unknown manifest
sections and fields are errors; only the verified local dependency form below is
accepted.

### Local dependencies and reproducible locks

A local package dependency has one explicit source and one import alias:

```toml
[dependencies]
math = { path = "../math" }
```

Ordinary source imports that package through its alias. An import of only the alias opens
the dependency entry module; remaining path components select modules inside that
dependency:

```disp
use math
use tools.statistics.{average}
```

Dependency aliases are valid lowercase DISP identifiers. If an alias would also name a
local module, compilation rejects the ambiguity rather than selecting one implicitly.
Dependencies may publicly re-export their own declared dependencies, but packages cannot
reach an undeclared transitive dependency directly.

Run `disp lock project` after reviewing a dependency or manifest change. The generated
`DISP.lock` records, in deterministic order:

```text
exact name and version identity
canonical relative source location
dependency alias edges
root manifest SHA-256
dependency source-tree SHA-256
```

The source digest covers the dependency manifest and all `.disp` files, normalizes CRLF
to LF for cross-platform checkout stability, and excludes build/VCS caches. Lock updates
use a temporary file, synchronization, and replace/rollback sequence. Normal `check`,
`build`, `run`, and `interpret` commands never rewrite the lockfile. Missing, manually
edited, stale, or content-mismatched lockfiles fail before dependency code enters the
compiler. `disp tree project` displays the exact locked graph.

Resolution rejects dependency cycles, duplicate name/version identities from different
source trees, symbolic links inside dependency source, graph/depth/file/byte limit
violations, malformed path specifications, and package import collisions. Git, registry,
remote archive, version-range, feature, and build-script dependencies remain unsupported
and fail closed; this pass implements verified local dependencies rather than pretending
that the future network package system already exists.

Shared function parameters also borrow named storage automatically:

```disp
fn count<T>(values: &List<T>) -> uint = (*values).len()
print(count(names))
```

Mutable references remain explicit because mutation must be visible at the call site.

## Paths, files, and time

Paths are nominal owned values rather than ordinary strings. This prevents accidentally
passing arbitrary text to filesystem operations:

```disp
folder = Path("reports")
file = folder.join("summary.txt")
File.write_text(file, "ready")?
text = File.read_text(file)?
```

Filesystem failures use `Result<_, IoError>` and therefore work with normal `?`
propagation. `Directory.remove` removes only an empty directory; recursive deletion is
intentionally not implicit. All native file and directory handles are closed before an
operation returns.

Time uses a monotonic `Instant` for elapsed measurements and an explicit `Duration`:

```disp
started = Time.now()
Time.sleep(Duration.from_millis(10))
print(started.elapsed().millis())
```

`Time.unix_seconds()` is the wall-clock operation. Keeping it separate prevents clock
adjustments from corrupting elapsed-time measurements.

`Time.ticks() -> u32` exposes a wrapping monotonic counter in fixed 10 millisecond units and carries
the distinct `Timer` effect. Hosted targets derive it from their monotonic provider. The x86-64
freestanding profile requires the function containing the operation to declare `uses Timer`, then
provides ticks through its bounded IRQ0 service.

## Programs, environment, and child processes

`fn main(args: List<String>)` receives only arguments supplied after the command-line
`--` separator. `Environment.arguments()` returns the same owned values, and
`Environment.get(name)` performs an explicit environment read.

`Process.run(path, arguments)` is the small direct-execution API. More control uses a
linear command value whose configuration methods consume it and return the configured
replacement:

```disp
fn invoke() -> Result<String, IoError> {
    command = Process.command(Path("tool"))
        .arg("--format=json")
        .directory(Path("workspace"))
        .environment("MODE", "safe")
        .input_text("request\n")
        .timeout(Duration.from_seconds(2))
    output = command.run()?
    return output.stdout_text()
}
```

Execution never invokes a command shell. Program paths and arguments stay separate,
working directories are nominal `Path` values, environment overrides are validated,
and standard input plus captured standard output/error are bounded to 16 MiB. A timeout
terminates and reaps the child. `ProcessOutput` exposes `status()`, `success()`, byte
`stdout()`/`stderr()`, and UTF-8-checking `stdout_text()`/`stderr_text()`.

Streaming uses an owned `ChildProcess`. Operations that change its state require `var`,
and `wait()` consumes it so a reaped process cannot be reused:

```disp
var child = command.start()?
child.write_text("request\n")?
first = child.read_stdout(1024)?
child.close_input()?
output = child.wait()?
```

`read_stdout(limit)` and `read_stderr(limit)` return available byte chunks, while
`try_wait()` reports `None` until the process exits. `kill()` is explicit. If a live
child leaves scope, DISP closes its input, terminates it, drains its pipes, reaps it,
and joins the internal readers before releasing the resource.

## Threads and synchronization

`spawn` transfers owned arguments to a named DISP function and returns a typed handle.
Joining consumes the handle and returns the function's result:

```disp
fn square(value: int) -> int = value * value

task = spawn square(12)
print(task.join())
```

References, borrowed views, raw pointers, and mutex guards cannot cross a thread
boundary. A `Thread<T>` that leaves scope without an explicit `join()` is joined during
deterministic cleanup, so it cannot outlive resources owned by the process.

`Mutex<T>` is explicitly shared. Its guard owns the lock and releases it when the guard
leaves scope. The mutex is recursive for its owning thread and becomes available to another
thread after the last nested guard is released:

```disp
counter = Mutex.new(0)
shared = counter.share()
guard = shared.lock()
*guard += 1
```

`AtomicInt` provides sequentially consistent `load`, `store`, `add`, and `fetch_add`
operations for counters that do not need a larger protected value. Explicit ordered forms
are also available: loads support `_relaxed`, `_acquire`, and `_seq_cst`; stores support
`_relaxed`, `_release`, and `_seq_cst`; `add` and `fetch_add` support all five relaxed,
acquire, release, acquire-release, and sequentially consistent suffixes. Invalid order and
operation combinations are absent and fail type checking. `share()` is explicit for both
synchronization types, keeping accidental shared ownership visible without exposing
reference-counting or platform handles. The complete contract is in `CONCURRENCY.md`.

Bounded `Channel<T>` queues transfer owned messages between threads with explicit backpressure:

```disp
var jobs: Channel<String> = Channel.bounded(64)?
worker = spawn consume(jobs.share())
jobs.send("compile")
jobs.close()
worker.join()
```

`send` consumes its message and blocks while the queue is full. `receive` blocks while an open
queue is empty and returns `Option<T>`; after `close`, buffered messages are drained before `None`.
Closing wakes all waiters, and the final handle deterministically drops messages still queued.

Async calls create lazy, linear `Future<T>` values. `Async.spawn` moves a future into a structured
`Task<T>`. Awaiting consumes the task and returns its result; explicit cancellation also consumes
the handle, while completion inspection is non-consuming:

```disp
task = Async.spawn(render(frame))
if task.is_finished() {
    print(await task)
} else {
    task.cancel()
}
```

Leaving a task unconsumed cancels it during deterministic scope cleanup. `cancel()` completes the
task's owned-state cleanup before returning. Timeouts use `Duration` and begin when the lazy
operation is first polled, not when its future is constructed. The full lifecycle contract and
Pass 016 audit matrix are in `ASYNC.md`.

A leading dereference assignment on the line after another expression is treated as a
new statement. This resolves the otherwise ambiguous `call()\n*guard += 1` spelling
without requiring a semicolon.

## C interoperability

Foreign functions are declarations inside an `extern C` block. A library name is
optional; when present, the native linker receives it as one validated library argument:

```disp
extern C {
    fn strlen(value: CStr) -> CSize
}

extern C("m") {
    fn sqrt(value: CDouble) -> CDouble
}
```

Calls require a narrowly scoped `unsafe` block because DISP can validate the ABI shape,
but cannot prove the contract implemented by foreign code:

```disp
owned = CString.new("checked UTF-8")?
view = owned.as_c_str()
unsafe {
    print(strlen(view))
    print(sqrt(81.0))
}
```

`CString.new` returns `Result<CString, String>` and rejects interior NUL bytes. The owned
value uses allocator-backed, deterministically released storage. `as_c_str()` is a
zero-copy `CStr` loan, so moving its owner, returning a view of a local owner, or sending
the view to another thread is rejected. `CStr` is accepted only as a parameter in the
defined ABI; borrowed C-string returns are rejected because an external declaration
cannot express a lifetime contract.

A checked DISP function may return a borrowed view when exactly one borrowed input
defines its elided lifetime. Returning a borrowed value from a function with multiple
possible borrowed origins is rejected until the source language has explicit lifetime
parameters.

The portable aliases are `CInt`, `CUInt`, `CSize`, `CSSize`, `CChar`, `CUChar`, `CShort`,
`CUShort`, `CLongLong`, `CULongLong`, `CFloat`, and `CDouble`. Fixed-width DISP numeric
types and explicit raw pointers are also ABI-safe. Owned DISP aggregates such as
`String`, `CString`, `List`, and ordinary user structs never cross the C boundary
implicitly. Plain value records can opt into a checked stable layout explicitly:

```disp
export C struct PacketHeader {
    flags: u8,
    payload_length: u32,
    sequence: u64,
}
impl Copy for PacketHeader {}

export C fn advance(header: PacketHeader) -> PacketHeader uses Pure {
    return PacketHeader {
        flags: header.flags,
        payload_length: header.payload_length,
        sequence: header.sequence + 1,
    }
}
```

The generated type is named `disp_c_PacketHeader`. Its C/C++ declaration retains the source
field names and carries compile-time assertions for every offset, its total size, and alignment.
C ABI records must be non-empty and non-generic and may contain only ABI scalars, explicit stable
raw pointers, or other exported C records; owned runtime values and private structs are rejected.
The fixed-record contract is compiled and inspected under both Windows x86-64 and i686 C calling
conventions; this is ABI evidence, not a claim that DISP currently ships an i686 runtime.

Native compilation can call any correctly declared linked symbol. The interpreter has
deterministic semantic-oracle support for `abs`, `strlen`, and `sqrt`; other foreign
functions produce a controlled diagnostic requiring native execution rather than
inventing foreign behavior.

`disp header app.disp` writes `app.h`; for a project directory it writes
`disp_ffi_v1.h`. The header is a deterministic description of every checked `extern C`
import and `export C fn` in the complete lowered program. It defines `DISP_C_ABI_VERSION` as `1`, uses
exact-width C types plus `intptr_t`/`uintptr_t`, emits stable typedefs for supported raw
pointers, includes C++ linkage guards, and rejects a raw pointer whose pointee has no
stable public C representation. Output is limited to 16 MiB and installed transactionally.
This header lets C and C++ compile against the exact contract DISP expects or provides.

A deliberately narrow first export profile is available for native embedding:

```disp
export C fn add(left: CInt, right: CInt) -> CInt uses Pure {
    return left + right
}
```

`disp build --library app.disp` produces a host shared library and `app.h`. An exported function
returns `DISP_C_STATUS_OK`, `DISP_C_STATUS_PANIC`, or `DISP_C_STATUS_INVALID_ARGUMENT`; a non-unit
DISP result is committed through the final `out_result` pointer only on success. Checked failures
are contained at the ABI boundary and described by the thread-local `disp_c_last_error()` string.
The header also declares an exact `disp_c_callback_add` function-pointer type for the example.
A C host may store and invoke the exported symbol through this type; it retains the same status,
out-result, and failure-containment contract.
Exports are currently synchronous, non-generic, explicitly authority-bounded, and limited to
ABI-safe scalars, explicit C records, raw pointers, and `CFunction` signatures. An export
normally declares explicit
`uses Pure`, or may declare exactly `uses Foreign` when it invokes a typed
context-free `CFunction`; all other authority is rejected. Until cleanup-aware failure containment
lands, its complete direct-call graph
must be allocation-free and cannot own managed storage, call indirectly or into runtime/data
intrinsics, spawn, or await. Synchronous scalar-only helpers and recursion are accepted after the
same restriction is proved transitively. C-to-DISP callbacks are supported through generated export
types. Same-thread nested foreign re-entry is denied. Each C host thread must call
`disp_c_thread_attach()` before entering an export and `disp_c_thread_detach()` after its final call.
Attachment and failure state are thread-local, so distinct attached C threads may enter concurrently.
Resource-owning exports and asynchronous callbacks are not implemented yet.

DISP can also hold and invoke a context-free C function pointer without confusing it with a closure:

```disp
extern C { fn abs(value: CInt) -> CInt }

fn invoke(callback: CFunction<fn(CInt) -> CInt>, value: CInt) -> CInt uses Foreign {
    unsafe uses Foreign {
        return callback(value)
    }
}
```

Only a named, non-generic `extern C` function with the exact signature becomes a `CFunction` value.
Invocation requires an explicit `Foreign` unsafe contract and checks null before entering C. The
pointer is Copy but currently thread-affine. DISP closures and ordinary functions never convert to
this type implicitly, and no closure environment crosses the ABI. Same-thread C→DISP→C→DISP re-entry
is denied before the nested body executes;
the inner wrapper returns `DISP_C_STATUS_INVALID_ARGUMENT` while the outer containment target remains
active.

DISP can adopt an opaque C context together with its exact release function:

```disp
extern C {
    fn acquire() -> mut ptr<Unit>
    fn release(context: mut ptr<Unit>)
}

fn use_provider() uses Foreign {
    unsafe uses Foreign {
        registration = CRegistration.adopt(acquire(), release)
        print(registration.is_active())
        // registration.close() may consume it early; otherwise scope exit releases it.
    }
}
```

`CRegistration` is non-Copy and thread-affine. `close()` consumes it, while native scope cleanup
releases an active registration exactly once. The callback is cleared and the handle is marked
inactive before provider code runs. Attaching a C thread does not make this thread-affine handle
transferable. For providers that may still have callbacks in flight, use
`CRegistration.adopt_async(context, quiesce, release)`. Cleanup clears the handle, invokes the exact
quiesce callback to stop and join provider work, and only then releases the context. This is an
explicit unsafe assertion that the foreign quiesce function really waits for every in-flight call.

A checked export wrapper can be passed to a C provider explicitly:

```disp
extern C {
    fn register(callback: CFunction<fn(CInt, mut ptr<CInt>) -> CInt>)
}
export C fn on_value(value: CInt) -> CInt uses Pure { return value + 1 }

fn install() uses Foreign {
    unsafe uses Foreign { register(CExport.callback(on_value)) }
}
```

The handle points to the status/out-result wrapper, not the internal DISP function. Ordinary
functions and closures are rejected. Provider threads must attach before invocation, and retained
providers must quiesce before library unload.

For a captured handler, registration and ownership transfer are atomic:

```disp
registration = CRegistration.register_async(
    move |value: CInt| value + offset,
    provider_register,
    provider_quiesce,
    provider_release
)
```

The provider register function receives a signature-specific checked trampoline plus an opaque
context. The linear registration owns moved Send-compatible captures, including resource-owning
values such as `String`; borrowed views, pointers, secrets, guards, functions, and registrations are
rejected. Every invocation borrows the reusable environment. Cleanup first quiesces all provider
calls, then recursively drops the capture environment, then releases the provider context. Handler
allocation and cleanup-bearing local work remain outside the current checked callback profile.

Checked C exports may create heap-only owned locals such as `String`. Export entry starts a
thread-local allocation transaction: ordinary return performs normal typed cleanup, while a
contained checked failure reclaims every still-owned managed allocation, restores call-depth
accounting, preserves the caller's output, and returns panic status. `CRegistration` also installs
a typed rollback hook immediately after acquisition. Contained failure invokes live hooks in reverse
order, including provider quiescence where required, before reclaiming allocation storage. Other
handles, tasks, threads, callable environments, and secrets remain rejected until each has an
explicit type-specific rollback hook.

## System memory

`Memory` is an owned, zero-initialized byte allocation. Size and alignment are explicit,
but allocation ownership and platform allocator details remain encapsulated:

```disp
memory = Memory.allocate(4096, 64)?
memory.write(0, u8(42))
print(memory.read(0))
memory.fill(u8(0))
```

The alignment must be a non-zero power of two no larger than 1 MiB. Invalid alignment or
size overflow is a recoverable `Result<Memory, String>` error. `read`, `write`, `fill`,
and `copy_from` are safe, bounds-checked operations; copying uses overlap-safe semantics.
The allocation is released deterministically on normal scope exit, return, `?`
propagation, and other compiler-generated control-flow cleanup paths.

`Memory.as_ptr()` and `Memory.as_mut_ptr()` create checked `MemoryPtr<u8>` and
`MemoryMutPtr<u8>` views without transferring allocation ownership. Their arithmetic,
reads, and writes remain explicit and require a `RawMemory`-bounded unsafe region:

```disp
unsafe uses RawMemory {
    pointer = memory.as_mut_ptr()
    pointer.write(u8(7))
    pointer.offset(1).write(u8(8))
}
```

Checked pointer operations accept only `Copy` element types. A checked pointer is a fat
value containing the current address, allocation base and byte length, and its element
size and alignment contract. Offsets may reach one-past the allocation but cannot leave
it; reads and writes reject one-past, incomplete, or misaligned access before native C
performs a dereference. The ownership checker keeps the source `Memory` loan live through
copies, offsets, aggregates, assignments, and direct calls, rejects owner movement or
conflicting access, and prevents a pointer from escaping its allocation lifetime.

`MemoryPtr<T>` is shared and `MemoryMutPtr<T>` is exclusive. Neither checked pointer kind
can cross a thread or C ABI boundary. They are distinct from thin `ptr<T>` / `mut ptr<T>`,
which exist for explicitly trusted foreign-memory contracts and receive no implicit
conversion from owned DISP memory. Unsafe capability containment remains active for both;
safe `Memory` methods remain the simplest interface.

## Legacy SQLite compatibility boundary

`Database` is a preview-only owned non-Copy SQLite compatibility connection. The bootstrap resolves
the system library lazily only after explicit construction, so the compiler and native DataStore do
not statically depend on SQLite. It is not the DISP Data engine and is scheduled to become an
optional isolated connector before 1.0. File paths remain nominal, while an
in-memory database needs no configuration:

```disp
fn store() -> Result<uint, DataError> {
    var database = Database.memory()?
    var none: List<Json> = List.new()
    database.execute("CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT)", none)?
    values = List.of(Json.string("safe input")?)
    inserted = database.execute("INSERT INTO notes(text) VALUES(?)", values)?
    rows = database.query("SELECT id, text FROM notes", none)?
    database.close()?
    return Ok(inserted)
}
```

`Database.open(Path)` and `Database.memory()` return `Result<Database, DataError>`.
`execute` and `query` require a mutable connection and a `List<Json>` of bound values;
ordinary code never has to concatenate data into SQL. `query` returns one owned `Json`
object per row. SQL `NULL`, integers, finite floats, and UTF-8 text map directly to JSON.
Duplicate column names and BLOB columns are rejected until an explicit byte-column API
can represent them without guessing.

Only one prepared statement is accepted per operation. SQL is bounded to 1 MiB, results
to 100,000 rows, 4096 columns, and 16 MiB of JSON. `begin`, `commit`, and `rollback`
track one explicit transaction and reject nesting or completion without an active
transaction. `close()` consumes the connection. Dropping a live connection first rolls
back an active transaction and then closes it, including compiler-generated error paths.

## DISP Data language

Persistent nominal records use `data`. Exactly one non-optional signed integer or
`String` field is marked `primary`:

```disp
data User {
    id: int primary
    name: String unique
    group: String index
    active: bool
    note: Option<String>
    constraint user_name: unique(group, name)
    constraint active_group: index(group, active)
}
```

DISP Data operations are language expressions, not SQL strings or methods copied from
a database API:

```disp
var store = data memory?
data add User { id: 1, name: "Ada", active: true, note: None } in store?

wanted = "Ada"
users = data find User in store
    where active && name == wanted
    order id descending
    limit 20?

data save User { id: 1, name: "Ada Lovelace", active: true, note: None } in store?
data remove User in store where id == 1?
```

Schema names, fields, condition types, ordering keys, limits, values, and store types
are checked by the compiler. Conditions become typed HIR data expressions and plans;
MIR carries a `DataPlanId` rather than embedded SQL. External values are evaluated once
and bound as parameters. `remove` requires `where`, preventing an accidental unbounded
delete in ordinary syntax. Limits are restricted to 100,000 rows.

`data memory` creates an ephemeral `DataStore`. It uses DISP's own native typed row
catalog and executes checked plans directly, including filtering, ordering, limits,
primary-key insertion/upsert, and guarded removal. External values are evaluated once.
Required fields may be marked `unique`; collisions return `DataError` and roll back the complete
operation. Optional unique fields are rejected until their absent-value semantics are standardized.
Fields marked `index` maintain non-unique equality indexes, so repeated values are valid and matching
queries return every row. These indexes share the same transactional mutation, validation, and
durable-reopen guarantees as primary and unique indexes.
Multi-field rules remain compact: `constraint tenant_handle: unique(tenant, handle)` rejects only a
repeated pair, while `constraint tenant_status: index(tenant, status)` permits duplicates and speeds
conjunctive equality queries. Named constraints are type-checked, persisted, and maintained directly
by DISP in interpreter and native execution.
`DataStore` and `Database` are distinct nominal types, so a data store cannot invoke raw
SQL methods.

PostgreSQL support belongs to an optional typed connector rather than `DataStore` internals.
Connector use requires explicit database/network capabilities; native `DataStore` programs retain
identical semantics when PostgreSQL and every other external database connector are absent.

`data open Path(...)` creates or opens a durable `DataStore` backed by DISP's native v3
storage format. The file is split into fixed 4096-byte pages with header, page, and payload
integrity checks. Each mutation commits only changed pages through a synced write-ahead log;
opening the store rolls a committed log forward after interruption. An operating-system lock
prevents a second process or store from mutating the same path concurrently, and the lock is
released deterministically when the store is dropped. Version 1 and version 2 snapshots remain readable and
migrate on their next successful mutation. Interpreter and native execution share the exact
format and recovery behavior.

## Secure operating-system randomness

```disp
fn nonce() -> Result<List<u8>, CryptoError> uses Random {
    return Crypto.random_bytes(12)
}
```

`Crypto.random_bytes` returns between 1 and 1,048,576 bytes from the operating system's secure
random provider. It returns a typed error if the length is invalid or the provider fails. Calling it
requires the `Random` capability; an omitted function contract infers that capability, while a
`uses Pure` contract is rejected. The native backend uses `BCryptGenRandom` on Windows and the
`getrandom` system call on Linux, with no deterministic fallback.

The returned `List<u8>` is deliberately public byte material suitable for nonces, salts, identifiers,
and challenges. Secret material uses an opaque source type instead:

```disp
fn key() -> Result<SecretBytes, CryptoError> uses Random {
    return Crypto.random_secret(32)
}
```

`SecretBytes` is owned and non-Copy, cannot cross a spawned-thread boundary, cannot be indexed,
serialized, extracted, directly printed, or compared with `==`/`!=`, and exposes only `len()`,
`is_empty()`, and `constant_time_equals(other)`. Nested formatting is always redacted. Both engines
enforce the same 1 through 1,048,576-byte range; native cleanup zeroizes the allocation before
release and interpreter cleanup delegates to the zeroizing bootstrap secret owner.

Public byte storage can be deliberately transferred into opaque storage with
`Crypto.import_secret(bytes)`. The input is consumed, and failed imports are wiped before release;
the operation cannot erase any earlier copies made by the program.

SHA-256 and keyed authentication are Pure operations:

```disp
fn authenticate(key: SecretBytes, message: List<u8>) -> Result<bool, CryptoError> uses Pure {
    authenticator = Crypto.hmac_sha256(key, message)?
    return Crypto.hmac_sha256_verify(key, message, authenticator)
}
```

`Crypto.sha256`, `Crypto.hmac_sha256`, and `Crypto.hmac_sha256_verify` borrow their inputs and cap
messages at 16 MiB. HMAC verification is the supported authenticator comparison path. Native
programs delegate to Windows CNG or Linux AF_ALG; provider failure remains a typed `CryptoError`.

Key derivation uses `Crypto.hkdf_sha256(salt, input, info, output_length)`. Salt and info are public
`List<u8>` values, input key material is borrowed `SecretBytes`, and output is a new opaque
`SecretBytes`. Empty salt selects the RFC 5869 default. Salt and info are limited to 1 MiB each and
output length to 1 through 8,160 bytes. Derivation is Pure and uses the same native HMAC providers;
all intermediate key material is zeroized before release.

## Collection loops

```disp
for name in names {
    print(name)
}
```

Copyable elements are read by value. Owned non-Copy elements are borrowed for the
iteration, and mutation or movement of the collection is rejected while that loan is
active.

The same loop works for sets and borrowed collection views. Maps expose `keys()` and
`values()` views so the loop stays unambiguous:

```disp
for name in scores.keys() {
    print(name)
}
```

Map keys and Set elements currently accept integers, `bool`, `char`, `String`, and
`str`. This makes equality deterministic without exposing hashing or allocator details
in ordinary code.

## Automatic JSON conversion

Ordinary nominal data can be converted without reflection, annotations, or handwritten
field plumbing:

```disp
struct User { id: uint, name: String }

document = Json.from(User { id: 7, name: "Ada" })?
user = User.from_json(document)?
```

Both operations return `Result<_, ConversionError>`. The compiler generates a concrete
codec for the exact native type. Structs are JSON objects with exactly their declared
fields; decoding rejects missing and unknown fields. Unit enum variants are strings,
one-field payload variants are single-member objects, and multi-field payloads use an
array inside that member. `Result` uses `{"Ok": value}` or `{"Err": value}`.
`Option<T>` uses `null` for `None` and the ordinary value for `Some`; element types that
can themselves encode as `null` are rejected so decoding is never ambiguous.

Fixed arrays, `List<T>`, `Map<String, T>`, integers, finite floats, `bool`, `char`,
`String`, `Json`, and nested nominal types participate recursively. Integer widths,
fixed-array lengths, Unicode scalar validity, JSON depth/size limits, and schema shape
remain checked. Generic types can be nested with concrete arguments; a generic nominal
type cannot yet be named as the static decoding owner, so a concrete wrapper supplies
that boundary.

## Concise functions and data

```disp
struct Point { x: int, y: int }
fn point(x: int, y: int) -> Point = Point { x, y }
```

The expression after `=` is an ordinary checked return expression. Field shorthand is
only accepted when a local with the same name supplies that field.
