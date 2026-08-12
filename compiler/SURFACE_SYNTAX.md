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

## Concise functions and data

```disp
struct Point { x: int, y: int }
fn point(x: int, y: int) -> Point = Point { x, y }
```

The expression after `=` is an ordinary checked return expression. Field shorthand is
only accepted when a local with the same name supplies that field.
