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
leaves scope:

```disp
counter = Mutex.new(0)
shared = counter.share()
guard = shared.lock()
*guard += 1
```

`AtomicInt` provides sequentially consistent `load`, `store`, `add`, and `fetch_add`
operations for counters that do not need a larger protected value. `share()` is explicit
for both synchronization types, keeping accidental shared ownership visible without
exposing reference-counting or platform handles.

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
`String`, `CString`, `List`, and user structs never cross the C boundary implicitly.

Native compilation can call any correctly declared linked symbol. The interpreter has
deterministic semantic-oracle support for `abs`, `strlen`, and `sqrt`; other foreign
functions produce a controlled diagnostic requiring native execution rather than
inventing foreign behavior.

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

Raw pointer views are available without transferring ownership. Pointer arithmetic,
reads, and writes remain explicit and require `unsafe`:

```disp
unsafe {
    pointer = memory.as_mut_ptr()
    pointer.write(u8(7))
    pointer.offset(1).write(u8(8))
}
```

Raw pointer operations currently accept only `Copy` element types. Raw pointers are not
sent across thread boundaries, and owned DISP values never become raw allocations
implicitly. Once code enters `unsafe`, it is responsible for pointer lifetime,
alignment, and bounds; safe `Memory` methods remain the preferred interface.

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
