# DISP Memory Model

> **Design draft:** GPT-generated and not authoritative. See [the documentation index](../README.md) for current, test-backed behavior.

## 0. Status

This document defines the initial memory-safety architecture for DISP.

The design is experimental and may evolve before DISP reaches a stable specification.

---

# 1. Goal

DISP must provide:

- memory safety
- predictable performance
- deterministic cleanup
- low runtime overhead
- simple everyday programming
- explicit low-level control when required

The programmer should not need to manually manage memory for ordinary DISP programs.

At the same time, DISP must remain suitable for operating systems, embedded software, servers, games, AI workloads, and other performance-critical software.

---

# 2. Core Memory Principle

> Safe by default. Deterministic by default. Explicit when control is required.

Safe DISP must prevent:

- use-after-free
- double-free
- dangling references
- invalid dereferencing
- accidental uninitialized reads
- unsafe aliasing
- data races
- iterator invalidation bugs
- invalid concurrent mutation

These guarantees must be enforced through the language, compiler, and standard library.

---

# 3. Ownership

Every owned value has one logical owner.

Example:

```disp
let data = Buffer.new(1024)
```

`data` owns the buffer.

When the owner reaches the end of its lifetime, the resource is released automatically.

```disp
fn example() {
    let data = Buffer.new(1024)

    use(data)
}
```

The programmer does not manually call `free()`.

---

# 4. Automatic Resource Cleanup

Resources use deterministic destruction.

Resources may include:

- heap memory
- files
- sockets
- locks
- database connections
- GPU buffers
- operating-system handles

Example:

```disp
fn read_config() {
    let file = File.open("config.disp")

    let data = file.read()
}
```

When `file` leaves its lifetime, the resource is automatically closed unless ownership has moved elsewhere.

---

# 5. Move Semantics

Large or resource-owning values may be transferred without copying.

```disp
let a = Buffer.new(1024)

let b = move a
```

After ownership moves, `a` cannot be used unless it is assigned another valid value.

The compiler must detect invalid use after move.

---

# 6. Copy Types

Small value types may implement copy semantics.

Examples may include:

```disp
i32
u64
f64
bool
char
```

Example:

```disp
let a = 10
let b = a
```

Both values remain valid.

Copy behavior must be defined by the type system rather than hidden compiler heuristics.

For DISP 1.0, scalar primitives and references are intrinsically `Copy`. A user-defined
struct or enum is `Copy` only with an explicit `impl Copy for Type {}` marker, and that
marker is rejected unless every stored field or payload is itself `Copy`.

Moving a field partially moves its enclosing aggregate. Other initialized fields remain
usable, but the aggregate as a whole is unavailable until the moved field is
reinitialized. Moving non-`Copy` data out through a reference is not permitted.

---

# 7. References

DISP supports non-owning references.

Immutable reference:

```disp
let value = 10
let ref = &value
```

Mutable reference:

```disp
var value = 10
let ref = &mut value
```

References do not own the referenced value.

The compiler must guarantee that references never outlive valid storage.

---

# 8. Borrowing

Borrowing temporarily grants access without transferring ownership.

Example:

```disp
fn print_name(name: &str) {
    print(name)
}
```

Calling:

```disp
let name = "DISP"

print_name(&name)
```

does not transfer ownership of `name`.

---

# 9. Simple Borrowing Rules

DISP should preserve memory safety without exposing unnecessary lifetime syntax in ordinary programs.

Conceptually:

```text
many readers
OR
one writer
```

Multiple immutable references may coexist.

Exclusive mutable access must not overlap with other active access that could create unsafe mutation.

The compiler should infer lifetimes automatically wherever possible.

---

# 10. No Mandatory Lifetime Annotations

Normal DISP code should not require explicit lifetime parameters.

Example:

```disp
fn first(items: &List<str>) -> &str {
    items[0]
}
```

The compiler should infer the relationship between input and output references when it is unambiguous.

Explicit lifetime controls may exist only for advanced cases where inference cannot safely determine intent.

---

# 11. Regions

DISP may internally use lexical and inferred memory regions.

Example:

```disp
{
    let buffer = Buffer.new(4096)

    process(&buffer)
}
```

The compiler knows the buffer cannot be referenced after the region ends.

Regions are primarily a compiler concept and should normally remain invisible to the programmer.

---

# 12. Heap Allocation

Heap allocation must be explicit at the semantic level even when syntax remains convenient.

Examples of heap-backed types may include:

```disp
List<T>
Map<K, V>
String
Box<T>
Shared<T>
```

The implementation must not silently introduce expensive heap allocations into operations expected to be zero-cost without documentation or diagnostics.

---

# 13. Stack Allocation

Values should use stack allocation when their size and lifetime permit it.

Example:

```disp
let point = Point {
    x: 10
    y: 20
}
```

The compiler may optimize storage placement provided observable semantics remain unchanged.

---

# 14. Escape Analysis

The compiler should perform escape analysis.

Example:

```disp
fn compute() -> i32 {
    let x = 10
    x * 2
}
```

No heap allocation should be required.

Objects may remain on the stack when the compiler proves they do not escape their scope.

---

# 15. Shared Ownership

Some programs require multiple owners.

DISP may provide explicit shared ownership:

```disp
let state = Shared.new(AppState())
```

Shared ownership must not be the default.

Reference counting or other runtime mechanisms used by `Shared<T>` must remain visible in the type and performance model.

---

# 16. Weak References

Shared ownership systems should support non-owning weak references where cycles are possible.

```disp
let weak = Weak.of(shared)
```

Weak references must not keep the underlying object alive.

---

# 17. Garbage Collection

DISP must not require a global tracing garbage collector for ordinary native programs.

However, specialized managed environments may optionally use garbage-collected memory when beneficial.

Such behavior must be explicit.

Conceptual example:

```disp
gc {
    ...
}
```

A GC-enabled feature must not silently change the memory model of unrelated code.

---

# 18. Arena Allocation

DISP should provide first-class arenas for workloads where many objects share the same lifetime.

Example:

```disp
arena temp {
    let a = temp.alloc(Node())
    let b = temp.alloc(Node())
}
```

All arena allocations can be released together when the arena ends.

This is useful for:

- compilers
- parsers
- games
- request processing
- temporary graphs
- AI workloads

---

# 19. Custom Allocators

Low-level DISP must support custom allocators.

Conceptually:

```disp
let buffer = Buffer.new(
    size: 4096,
    allocator: my_allocator
)
```

This is required for:

- operating systems
- embedded systems
- real-time systems
- games
- high-performance computing
- specialized memory hardware

---

# 20. Null Safety

Normal references cannot be null.

```disp
let user: &User
```

means a valid reference.

Optional values must be explicit:

```disp
let user: Option<&User>
```

Example:

```disp
match user {
    Some(value) => print(value.name)
    None => print("No user")
}
```

Null-pointer dereferencing must not exist in safe DISP.

---

# 21. Bounds Safety

Array, slice, and collection access must be bounds-safe.

```disp
let value = items[index]
```

If the compiler cannot prove the access is valid, safe behavior must be guaranteed.

Unchecked access must require an explicit unsafe operation.

```disp
unsafe {
    let value = items.get_unchecked(index)
}
```

---

# 22. Initialization Safety

Variables cannot be read before initialization.

Invalid:

```disp
let x: i32

print(x)
```

The compiler must reject this program.

---

# 23. Integer Safety

Arithmetic behavior must be defined.

DISP must not rely on undefined integer overflow.

Safe arithmetic should have explicitly defined behavior.

Checked operations should be available:

```disp
let result = a.checked_add(b)
```

Wrapping behavior must be explicit:

```disp
let result = a.wrapping_add(b)
```

Saturating behavior must also be explicit:

```disp
let result = a.saturating_add(b)
```

---

# 24. Pointers

Raw pointers exist only for low-level interoperability and systems programming.

Conceptually:

```disp
ptr<T>
mut ptr<T>
```

Dereferencing raw pointers requires unsafe context.

```disp
unsafe {
    let value = *pointer
}
```

Safe references and raw pointers are distinct types.

---

# 25. Unsafe Memory

Unsafe DISP allows operations the compiler cannot fully verify.

Examples include:

- raw pointer dereferencing
- manual allocation
- FFI
- memory-mapped hardware
- unchecked indexing
- explicit aliasing
- low-level memory reinterpretation

Example:

```disp
unsafe {
    ...
}
```

Unsafe code does not mean unchecked chaos.

The programmer becomes responsible for maintaining DISP's required invariants within that boundary.

---

# 26. Unsafe Isolation

Unsafe operations should be narrow.

Preferred:

```disp
fn safe_api() -> Value {
    unsafe {
        low_level_operation()
    }
}
```

Rather than:

```disp
unsafe fn entire_application() {
    ...
}
```

Safe callers should interact with safe abstractions whenever possible.

---

# 27. Concurrency Safety

Safe DISP must prevent data races.

Mutable memory shared between threads must use synchronization or another compiler-approved mechanism.

Example:

```disp
let counter = Atomic<i64>(0)
```

or:

```disp
let state = Mutex(AppState())
```

Unsynchronized shared mutation must not compile.

---

# 28. Send and Share Properties

Types may have compile-time concurrency properties.

Conceptually:

```text
Send
Share
```

`Send` means ownership may safely move between execution contexts.

`Share` means references may safely be shared concurrently.

The compiler should derive these properties automatically whenever possible.

---

# 29. Structured Concurrency

Concurrent tasks should normally remain bound to a parent scope.

Example:

```disp
task.group {
    spawn download_a()
    spawn download_b()
}
```

The group should not complete until its child tasks complete or are explicitly cancelled.

This prevents accidental orphan tasks and resource leaks.

---

# 30. Data-Race Freedom

Safe DISP programs must be data-race free.

This guarantee applies to:

- threads
- asynchronous tasks
- parallel loops
- shared containers
- reference access

Low-level opt-outs require explicit unsafe code.

---

# 31. GPU Memory

DISP should model GPU and accelerator memory explicitly enough to preserve performance.

Conceptually:

```disp
let data = gpu.buffer<f32>(1_000_000)
```

Transfers between host and accelerator memory should be visible when they have meaningful runtime cost.

The compiler may eliminate transfers when it can prove them unnecessary.

---

# 32. Zero-Copy Operations

DISP should support zero-copy views.

Example:

```disp
let view = buffer.slice(100..200)
```

A view should borrow existing storage rather than allocating and copying unless explicitly requested.

---

# 33. Slices

Slices represent borrowed contiguous memory.

Conceptually:

```disp
Slice<T>
MutSlice<T>
```

Example:

```disp
fn sum(values: Slice<i32>) -> i32 {
    ...
}
```

Slices carry enough metadata to enforce bounds safety.

---

# 34. Strings

DISP should distinguish ownership and views for strings.

Conceptually:

```disp
String
str
```

`String` owns dynamically allocated text.

`str` represents a borrowed string view.

Unicode behavior must be explicitly defined by the final text specification.

---

# 35. Resource Types

Types may define deterministic destruction behavior.

Conceptually:

```disp
struct File {
    ...
}

drop File {
    close(self)
}
```

Destructors must run predictably when ownership ends unless ownership has been intentionally leaked or transferred.

The ownership analysis records explicit drop facts for every live owning slot on normal
and early exits, in reverse declaration order. The tree-walking runtime performs the same
reverse-order scope teardown and skips moved storage. Executable user-defined `drop`
bodies are intentionally deferred until HIR/MIR can represent and validate destructor
control flow; the conceptual `drop Type` syntax above is therefore not yet accepted.

---

# 36. Explicit Leaking

Deliberately leaking memory or resources must be explicit.

Conceptually:

```disp
unsafe {
    leak(value)
}
```

Safe DISP should never accidentally require memory leaks to satisfy the type system.

---

# 37. Foreign Memory

Memory obtained from external libraries must be wrapped behind explicit interoperability boundaries.

Example:

```disp
unsafe {
    let ptr = c.malloc(size)
}
```

Safe DISP abstractions should then manage the pointer's lifetime where possible.

---

# 38. Real-Time Programming

DISP should support environments where unpredictable pauses are unacceptable.

Therefore:

- garbage collection is not mandatory
- deterministic destruction is supported
- heap allocation can be avoided
- allocators can be replaced
- bounded operations can be selected
- hidden runtime pauses should be minimized

---

# 39. Embedded Programming

DISP must support programs without a full operating system.

Potential environments include:

```text
no OS
no heap
no filesystem
no threads
limited RAM
limited flash
```

The language must allow these capabilities to be disabled without making the core language unusable.

---

# 40. Memory Profiles

DISP may support explicit compilation profiles.

Conceptually:

```text
native
embedded
realtime
managed
gpu
web
```

Different profiles may restrict available runtime features while preserving the same core language semantics.

---

# 41. Compiler Responsibilities

The DISP compiler should perform:

- ownership analysis
- borrow analysis
- lifetime inference
- escape analysis
- alias analysis
- bounds-check elimination
- dead allocation elimination
- stack promotion
- move optimization
- copy elision
- SIMD optimization
- resource-lifetime checking

Safety optimizations must never alter program correctness.

---

# 42. Developer Experience

Memory-safety diagnostics must explain:

1. what operation is invalid
2. which value owns the resource
3. where ownership moved
4. which reference remains active
5. why the operation would be unsafe
6. a practical way to fix it

The compiler should teach rather than merely reject.

---

# 43. Complexity Rule

DISP must not expose internal compiler complexity unnecessarily.

The programmer should think primarily about:

```text
value
ownership
borrowing
sharing
mutation
```

rather than manually describing every inferred lifetime.

---

# 44. Performance Rule

Safety features must be designed so the compiler can eliminate checks when correctness can be proven statically.

Example:

```disp
for i in 0..items.len {
    process(items[i])
}
```

The compiler should be able to eliminate redundant bounds checks when the loop proves the index valid.

---

# 45. Safety Rule

No optimization may weaken DISP's defined safety guarantees.

A compiler optimization that changes correct program behavior is a compiler bug.

---

# 46. Memory Model Summary

The initial DISP memory architecture is:

```text
Single ownership by default
        +
Automatic deterministic cleanup
        +
Move semantics
        +
Compiler-inferred borrowing
        +
Non-null references
        +
Explicit shared ownership
        +
Optional arenas
        +
Optional managed memory
        +
Explicit unsafe boundaries
        +
Compile-time concurrency safety
```

The objective is to obtain the safety and predictability required for systems programming without forcing advanced memory-management syntax into ordinary application code.

---

# 47. DISP Memory Principle

> The compiler should carry the complexity whenever the compiler can prove the answer.

The programmer takes control only when the program genuinely requires it.

---

# DISP

**Data. Intelligence. System. Page.**

**Safe memory. Predictable performance. Minimal friction.**
