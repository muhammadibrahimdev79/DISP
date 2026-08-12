# DISP Language Specification

> **Design draft:** GPT-generated and not authoritative. See [the documentation index](../README.md) for current, test-backed behavior.

## 0. Status

DISP is an experimental programming language under active design.

This specification defines the intended semantics of DISP before implementation decisions are locked.

### 0.1 Specification precedence and stability ledger

As of August 9, 2026, no complete DISP 1.0 feature set is marked stable. A feature is not stable merely because it appears in a design example or is recognized by the lexer. The implementation and release notes must therefore describe completed subsets precisely and must not claim DISP 1.0 conformance yet.

When this overview conflicts with a specialized design document, the specialized document governs that subject. In particular, the ownership direction in `MEMORY.md` supersedes the older statement below that the ownership model remains wholly unspecified. Specialized documents remain experimental unless a section is explicitly marked stable.

Implementation work proceeds in the dependency order in `ROADMAP.md`. Unsupported specified syntax must be rejected explicitly; it must not be accepted with placeholder semantics.

---

# 1. Design Goals

DISP is designed to be:

1. **Fast** — native, predictable performance.
2. **Easy** — simple syntax and excellent developer experience.
3. **Secure** — safety enforced by default.
4. **All-purpose** — one language across major computing domains.

DISP stands for:

- **Data**
- **Intelligence**
- **System**
- **Page**

---

# 2. Core Principle

> Easy by default. Powerful when necessary. Safe by default. Fast everywhere.

DISP must not depend on undefined behavior for normal safe programs.

Low-level operations that cannot satisfy DISP's normal safety guarantees must be explicitly isolated.

---

# 3. Source Files

DISP source files use:

```text
.disp
```

Example:

```text
main.disp
```

The default program entry point is:

```disp
fn main() {
    print("Hello, DISP!")
}
```

---

# 4. Variables

Immutable bindings are the default:

```disp
let name = "DISP"
let version = 1
```

Mutable bindings must be explicit:

```disp
var counter = 0

counter += 1
```

---

# 5. Primitive Types

Initial primitive types:

```disp
bool

i8
i16
i32
i64
i128

u8
u16
u32
u64
u128

f32
f64

char
str
```

Machine-sized integers:

```disp
int
uint
```

Exact widths must be used when representation matters.

---

# 6. Functions

```disp
fn add(a: i32, b: i32) -> i32 {
    return a + b
}
```

Expression returns may also be supported:

```disp
fn add(a: i32, b: i32) -> i32 {
    a + b
}
```

Type inference should remove unnecessary annotations without weakening static checking.

---

# 7. User-Defined Types

Structures:

```disp
struct User {
    id: u64
    name: str
}
```

Enumerations:

```disp
enum Status {
    Active
    Disabled
    Deleted
}
```

DISP should support algebraic data types and exhaustive pattern matching.

---

# 8. Error Handling

Recoverable errors should be represented explicitly.

```disp
fn load(path: str) -> Result<File, IOError> {
    ...
}
```

Errors must not silently disappear.

DISP should avoid exceptions as invisible control flow in ordinary code.

---

# 9. Memory Model

Safe DISP must guarantee memory safety.

The language must prevent:

- use-after-free
- double-free
- dangling references
- invalid memory access
- data races
- unsafe aliasing
- accidental uninitialized memory access

The final ownership and lifetime model remains to be specified.

It must target predictable performance without requiring programmers to fight the compiler for ordinary programs.

---

# 10. Unsafe Operations

Operations outside DISP's safety guarantees must be explicit:

```disp
unsafe {
    ...
}
```

Unsafe code must not disable safety checking for unrelated code.

The unsafe surface should remain minimal and auditable.

---

# 11. Concurrency

DISP will provide first-class concurrency.

```disp
async fn fetch() {
    ...
}

let result = await fetch()
```

The type system and runtime model should prevent data races in safe DISP.

Structured concurrency should be preferred over detached unmanaged tasks.

---

# 12. Data

Database and data operations are first-class language capabilities.

Conceptual example:

```disp
let adults = users
    .where(age >= 18)
    .select(name, email)
```

Queries should remain typed and compiler-checkable.

DISP should not require constructing SQL strings for ordinary database operations.

---

# 13. Intelligence

DISP will support numerical, AI, ML, SIMD, GPU, and accelerator workloads.

Conceptual example:

```disp
tensor x = load("input")

let y = model(x)
```

Hardware acceleration should integrate with the normal type system rather than requiring an unrelated programming language.

---

# 14. System

DISP must support low-level programming.

Targets should eventually include:

- operating systems
- drivers
- embedded systems
- servers
- networking
- native libraries
- command-line software
- performance-critical applications

The language must permit explicit control over memory layout and allocation when required.

---

# 15. Page

User interfaces are first-class DISP programs.

Conceptual syntax:

```disp
page Home {
    text("Hello, DISP!")
}
```

Styling:

```disp
style Home {
    width: 100%
    align: center
}
```

Behavior, structure, and styling should share one coherent language and type system.

---

# 16. Compilation

DISP should support ahead-of-time native compilation.

Potential targets include:

```text
x86-64
ARM64
WebAssembly
GPU targets
embedded targets
```

The compiler architecture must remain backend-independent where practical.

---

# 17. Performance

DISP should pursue:

- zero-cost abstractions
- predictable allocation
- efficient data layouts
- SIMD
- vectorization
- multithreading
- compile-time specialization
- dead-code elimination
- link-time optimization
- GPU acceleration

Performance claims must be benchmarked rather than assumed.

---

# 18. Security

Safe DISP should provide strong defaults including:

- memory safety
- type safety
- bounds checking
- safe concurrency
- explicit unsafe boundaries
- dependency integrity
- secure package verification
- compiler diagnostics for dangerous operations

Security-sensitive behavior should be explicit and auditable.

---

# 19. Toolchain

The official toolchain is intended to use one primary command:

```bash
disp
```

Examples:

```bash
disp new
disp run
disp build
disp check
disp test
disp fmt
disp doc
disp bench
disp package
```

---

# 20. Compatibility

DISP should eventually provide practical interoperability with existing ecosystems.

Priority interoperability targets:

```text
C ABI
C++
Rust
Python
JavaScript
WebAssembly
native system APIs
```

Interoperability must not silently compromise DISP's safety guarantees.

---

# 21. Language Rule

Every DISP feature must justify itself against four requirements:

**Simplicity. Performance. Safety. Universality.**

A feature that introduces complexity without sufficient benefit should not become part of the language.

---

# 22. Specification Policy

DISP syntax and semantics are not considered final until explicitly marked stable.

Implementation must follow the specification.

The specification must not be rewritten merely to excuse compiler bugs.

Correctness comes before compatibility during the experimental phase.

---

# DISP

**Data. Intelligence. System. Page.**

**One language for computing.**
