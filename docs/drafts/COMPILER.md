# DISP Compiler Architecture

> **Design draft:** GPT-generated and not authoritative. See [the documentation index](../README.md) for current, test-backed behavior.

## 0. Status

This document defines the initial architecture of the DISP compiler.

The design is experimental until explicitly stabilized.

The compiler must prioritize:

- correctness
- security
- compilation speed
- runtime performance
- clear diagnostics
- portability
- deterministic builds
- modularity

---

# 1. Core Principle

> Parse simply. Prove aggressively. Optimize safely. Generate fast code.

The DISP compiler must never sacrifice language correctness for optimization.

---

# 2. Compiler Command

The primary compiler tool is:

```text
disp
```

Core commands:

```text
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

# 3. Compilation Pipeline

Initial compiler pipeline:

```text
Source
  ↓
Lexer
  ↓
Parser
  ↓
AST
  ↓
Name Resolution
  ↓
Type Checking
  ↓
Ownership Analysis
  ↓
Borrow Analysis
  ↓
Effect / Safety Analysis
  ↓
HIR
  ↓
MIR
  ↓
Optimization
  ↓
Backend IR
  ↓
Machine Code / WASM / GPU
  ↓
Linker
  ↓
Executable
```

---

# 4. Source Loading

The compiler begins by loading:

```text
.disp
```

files.

It must validate:

- encoding
- module structure
- package boundaries
- duplicate modules
- file integrity
- invalid source bytes

UTF-8 should be the default source encoding.

---

# 5. Lexer

The lexer converts source text into tokens.

Example:

```disp
let x = 10
```

becomes conceptually:

```text
LET
IDENTIFIER(x)
EQUAL
INTEGER(10)
```

The lexer must support:

- identifiers
- keywords
- integers
- floats
- strings
- characters
- operators
- punctuation
- comments
- attributes

---

# 6. Lexer Requirements

The lexer must:

- run in linear time
- report precise source positions
- reject malformed literals
- preserve useful diagnostic information
- avoid unsafe parser ambiguity

Tokens should contain:

```text
kind
value
source_file
start_position
end_position
```

---

# 7. Parser

The parser converts tokens into an Abstract Syntax Tree.

Example:

```disp
let x = 10
```

becomes conceptually:

```text
LetStatement
├── name: x
└── value:
    IntegerLiteral(10)
```

---

# 8. Parser Strategy

The initial parser should favor:

```text
recursive descent
+
precedence parsing
```

because DISP prioritizes:

- readable implementation
- strong diagnostics
- maintainability
- predictable grammar evolution

The grammar must remain deterministic.

---

# 9. Grammar

The language grammar should eventually be formally defined.

Possible notation:

```text
EBNF
```

Example:

```text
function :=
    "fn" identifier
    "(" parameters ")"
    return_type?
    block
```

The formal grammar must match actual compiler behavior.

---

# 10. AST

The Abstract Syntax Tree represents source-level structure.

Core AST nodes may include:

```text
Program

Module
Import

Function
Parameter
Block

Let
Var
Const

If
Match
For
While
Loop

Struct
Enum
Trait
Impl

Expression
Call
Binary
Unary

Data
Page
Component
Style
Route

Unsafe
Extern
Comptime
```

---

# 11. Source Locations

Every AST node should preserve source-location information.

This enables diagnostics such as:

```text
main.disp:12:8

expected i32
found String
```

Diagnostics must point to the exact relevant expression.

---

# 12. Name Resolution

After parsing, the compiler resolves identifiers.

Example:

```disp
let user = create_user()
print(user.name)
```

The compiler must determine exactly which:

```text
create_user
user
name
print
```

each reference means.

---

# 13. Symbol Tables

The compiler maintains symbol information for:

```text
variables
functions
types
traits
modules
constants
fields
methods
generics
imports
```

Each symbol should have a unique internal identity.

---

# 14. Scopes

Scopes must be explicitly tracked.

Example:

```disp
let x = 10

if true {
    let x = 20
}
```

The inner `x` is distinct from the outer `x`.

Invalid access outside a scope must fail.

---

# 15. Module Resolution

Modules must resolve deterministically.

Example:

```disp
use math.sqrt
```

The compiler must know exactly which package and module provide `sqrt`.

Ambiguous imports must be rejected.

---

# 16. Type Checker

The type checker verifies every statically typed expression.

Example:

```disp
let x: i32 = "hello"
```

must fail.

The checker must support:

- inference
- generics
- traits
- algebraic types
- references
- ownership
- numeric conversions
- function types
- closures
- async types
- data types
- page types
- GPU types

---

# 17. Type Inference

Inference should solve obvious types automatically.

Example:

```disp
let x = 10
```

The compiler determines the type without requiring:

```disp
let x: int = 10
```

Inference must never silently weaken type safety.

---

# 18. Constraint Solver

Generic and inferred types require constraint solving.

Example:

```disp
fn max<T: Ordered>(a: T, b: T) -> T
```

The compiler verifies that `T` implements:

```text
Ordered
```

Constraint solving must terminate predictably.

---

# 19. Trait Resolution

The compiler resolves trait implementations statically whenever possible.

Example:

```disp
impl Display for User
```

The compiler must detect:

- missing implementations
- conflicting implementations
- ambiguous implementations
- invalid method signatures

---

# 20. Ownership Analysis

Ownership analysis determines:

- who owns each resource
- when ownership moves
- when resources are destroyed
- whether a value is used after move

Example:

```disp
let a = Buffer.new(1024)
let b = move a

use(a)
```

must fail.

---

# 21. Borrow Analysis

The borrow checker validates references.

Conceptual rule:

```text
many readers
OR
one writer
```

It must prevent:

- dangling references
- unsafe aliasing
- mutable alias conflicts
- reference use after destruction
- invalid concurrent access

---

# 22. Lifetime Inference

The compiler should infer lifetimes internally.

Normal DISP code should avoid explicit lifetime annotations.

The compiler determines relationships between:

```text
owners
references
scopes
returns
closures
```

---

# 23. Region Analysis

Borrow checking may use internal regions.

Example:

```disp
{
    let value = Data()
    let ref = &value
}
```

The compiler proves the reference cannot escape beyond the lifetime of `value`.

---

# 24. Initialization Analysis

The compiler tracks whether values are initialized.

Invalid:

```disp
let x: i32

print(x)
```

This must fail before code generation.

---

# 25. Exhaustiveness Analysis

Pattern matches must be checked.

Example:

```disp
match status {
    Active => ...
    Disabled => ...
}
```

If `Deleted` exists, the compiler must report the missing case.

---

# 26. Bounds Analysis

The compiler should prove bounds statically where possible.

Example:

```disp
for i in 0..items.len {
    process(items[i])
}
```

A redundant runtime bounds check should be removable.

---

# 27. Numeric Analysis

The compiler must track:

- signedness
- width
- overflow semantics
- conversion safety
- compile-time constants

Dangerous implicit conversions must be rejected.

---

# 28. Concurrency Analysis

The compiler must enforce data-race freedom in safe DISP.

It should verify:

```text
Send
Share
mutation exclusivity
task ownership
shared state synchronization
```

Unsafe concurrent memory access requires explicit unsafe boundaries.

---

# 29. Effect Analysis

DISP may track important effects.

Potential effects:

```text
IO
Network
Database
Async
GPU
Unsafe
Filesystem
Process
```

This system may initially be limited and expanded gradually.

---

# 30. Capability Analysis

Security-sensitive operations may require capabilities.

Example:

```disp
fn delete(
    permission: FileDeleteCapability,
    path: Path
)
```

The compiler verifies that required authority exists.

---

# 31. HIR

After semantic analysis, DISP lowers source AST into:

```text
HIR
```

High-Level Intermediate Representation.

HIR removes syntax-only complexity while preserving high-level semantics.

HIR may normalize:

```text
loops
method calls
operators
pattern matching
sugar
pipelines
```

---

# 32. MIR

HIR lowers into:

```text
MIR
```

Mid-Level Intermediate Representation.

MIR should make:

```text
control flow
ownership
moves
borrows
drops
branches
calls
memory operations
```

explicit.

MIR becomes the main representation for optimization and safety validation.

---

# 33. Control-Flow Graph

Functions in MIR should be represented using basic blocks.

Example:

```text
Block0:
    if condition -> Block1 else Block2

Block1:
    ...
    -> Block3

Block2:
    ...
    -> Block3
```

This enables advanced analysis.

---

# 34. SSA

Optimized MIR may transition toward:

```text
Static Single Assignment
```

where useful.

SSA simplifies:

- constant propagation
- dead-code elimination
- value analysis
- register allocation
- vectorization

---

# 35. Drop Elaboration

Automatic destruction must be made explicit before lower-level code generation.

The compiler determines exactly where destructors execute.

Cleanup must happen correctly across:

```text
normal returns
early returns
error propagation
break
continue
task cancellation
```

---

# 36. Monomorphization

Generic functions may be specialized.

Example:

```disp
identity<i32>()
identity<f64>()
```

may produce distinct optimized machine code.

The compiler must balance:

```text
performance
vs
binary size
```

---

# 37. Devirtualization

Dynamic calls should be converted into static calls when the compiler can prove the target.

This may eliminate runtime dispatch overhead.

---

# 38. Constant Evaluation

Compile-time constants should be evaluated by the compiler.

Example:

```disp
const SIZE = 1024 * 1024
```

The runtime executable should contain the final constant value.

---

# 39. Comptime Engine

DISP may execute restricted code during compilation.

Example:

```disp
const TABLE = comptime generate_table()
```

The compile-time environment must be sandboxed.

It must not receive unrestricted access to:

```text
filesystem
network
process execution
environment secrets
```

without explicit permission.

---

# 40. Compile-Time Security

Compiler execution of user code is a security boundary.

Comptime code must have:

- resource limits
- deterministic behavior where possible
- explicit permissions
- memory limits
- execution limits
- restricted external access

---

# 41. Macro System

If DISP gains macros, they should operate on structured syntax rather than unrestricted text substitution.

Preferred direction:

```text
typed or syntax-tree macros
```

Avoid unsafe preprocessor-style textual replacement.

---

# 42. Optimization Levels

Possible build modes:

```text
-O0
-O1
-O2
-O3
-Os
-Oz
```

Higher optimization must never change defined program behavior.

---

# 43. Debug Mode

Debug builds prioritize:

- fast compilation
- diagnostics
- runtime checks
- source mapping
- debugging information

Example:

```text
disp build
```

may default to development settings.

---

# 44. Release Mode

Release builds prioritize runtime performance.

Example:

```text
disp build --release
```

Possible optimizations:

```text
inlining
vectorization
LTO
dead-code elimination
constant folding
specialization
bounds-check elimination
```

---

# 45. Optimization Passes

Initial optimization passes may include:

```text
constant folding
constant propagation
dead-code elimination
dead-store elimination
copy propagation
common subexpression elimination
inlining
loop simplification
loop invariant code motion
bounds-check elimination
escape analysis
stack promotion
copy elision
vectorization
devirtualization
```

---

# 46. Escape Analysis

The compiler determines whether values escape their local scope.

Example:

```disp
fn calculate() -> i32 {
    let x = 10
    x * 2
}
```

No heap allocation should occur.

---

# 47. Stack Promotion

Heap-like temporary allocations may be promoted to the stack if proven safe.

This optimization must preserve observable semantics.

---

# 48. Allocation Elimination

Temporary allocations should be removed where possible.

Example:

```disp
process(transform(value))
```

should not automatically allocate intermediate objects if unnecessary.

---

# 49. Copy Elision

Unnecessary copying should be eliminated.

Move semantics and ownership information give the compiler strong opportunities for this optimization.

---

# 50. Auto Vectorization

Normal numerical loops should be vectorized when safe.

Example:

```disp
for i in 0..values.len {
    values[i] *= 2.0
}
```

The compiler may generate:

```text
SIMD
AVX
NEON
```

depending on target hardware.

---

# 51. SIMD Backend

Explicit SIMD should also be supported.

Potential targets:

```text
SSE
AVX
AVX2
AVX-512
NEON
SVE
WASM SIMD
```

---

# 52. Parallelization

The compiler may parallelize explicitly marked operations.

Example:

```disp
parallel for item in items {
    process(item)
}
```

Automatic parallelization may occur only when correctness can be proven.

---

# 53. Backend Architecture

DISP should use a backend-independent compiler architecture.

Possible backends:

```text
Native CPU
WebAssembly
GPU
Embedded
JIT
```

The frontend and type system should not depend on one backend.

---

# 54. Native Backend

Native compilation targets include:

```text
x86-64
ARM64
RISC-V
```

Future architectures may be added without changing core language semantics.

---

# 55. LLVM

The first implementation may use LLVM as a native-code backend.

Advantages include:

```text
mature optimization
many CPU targets
debug information
linker ecosystem
vectorization
```

DISP must not permanently depend on LLVM semantics.

The language specification remains independent.

---

# 56. Future Native Backend

DISP may later develop its own native backend.

Possible reasons:

```text
faster compilation
greater optimization control
smaller toolchain
specialized DISP semantics
security auditing
```

This should happen only when justified.

---

# 57. WebAssembly Backend

DISP should support:

```text
wasm32
wasm64
```

where available.

Use cases:

```text
web applications
sandboxed execution
plugins
edge computing
portable applications
```

---

# 58. GPU Backend

DISP Intelligence code may compile to GPU targets.

Potential targets:

```text
SPIR-V
PTX
native GPU APIs
WebGPU
```

GPU compilation must remain integrated with the normal type system.

---

# 59. GPU Kernel Compilation

Example:

```disp
gpu fn add(a: Tensor<f32>, b: Tensor<f32>) -> Tensor<f32> {
    ...
}
```

The compiler should:

```text
validate device-safe code
analyze memory
generate kernel IR
optimize memory access
generate device code
```

---

# 60. Device Safety

GPU code must reject unsupported operations.

Examples may include:

```text
filesystem access
host-only pointers
unsupported allocation
invalid synchronization
```

The compiler should detect these before runtime.

---

# 61. Embedded Backend

DISP should support environments with:

```text
no OS
no allocator
no filesystem
no threads
no standard runtime
```

Possible command:

```text
disp build --target embedded
```

---

# 62. Freestanding Mode

A freestanding DISP program should be able to operate without the standard operating-system runtime.

Example target use cases:

```text
kernels
bootloaders
firmware
microcontrollers
```

---

# 63. Runtime

DISP should minimize mandatory runtime dependencies.

Basic native programs should not require a large VM or mandatory garbage collector.

Runtime services may include:

```text
panic support
async executor
allocation
threading
reflection
GC
```

only when used.

---

# 64. Pay-for-What-You-Use

Unused runtime functionality should not be linked into the final program.

Example:

A command-line program that does not use:

```text
GPU
GC
HTTP
Page
Database
Async
```

should not carry those systems.

---

# 65. Standard Library

The compiler and standard library must evolve together.

Core areas may include:

```text
core
memory
collections
text
math
io
filesystem
network
async
data
gpu
page
crypto
time
process
```

---

# 66. Core Library

A minimal:

```text
core
```

library should work without a full operating system.

It should provide fundamental functionality such as:

```text
primitive operations
Option
Result
iterators
basic traits
memory primitives
```

---

# 67. Package Compilation

Packages should be compiled as dependency graphs.

Example:

```text
application
├── networking
├── database
└── crypto
```

Independent modules may compile in parallel.

---

# 68. Incremental Compilation

DISP should support incremental compilation.

Changing one file should not require rebuilding the entire project when dependencies are unchanged.

The compiler should cache:

```text
parsed modules
type information
HIR
MIR
object files
dependency metadata
```

---

# 69. Parallel Compilation

The compiler should compile independent units concurrently.

Modern multi-core systems should be utilized automatically.

---

# 70. Dependency Tracking

The compiler must know which code depends on which definitions.

Changing:

```disp
fn add(a: i32, b: i32) -> i32
```

should invalidate only relevant dependent units.

---

# 71. Content Hashing

Compiler cache keys should use cryptographic or collision-resistant content hashes.

This allows reliable detection of changed inputs.

---

# 72. Deterministic Builds

Given identical:

```text
source
compiler
target
dependencies
configuration
```

the compiler should produce reproducible output where practical.

Build timestamps and nondeterministic metadata should not unnecessarily alter binaries.

---

# 73. Secure Compilation

The compiler must treat source code as untrusted input.

It must defend against:

```text
malformed syntax
deep recursion
pathological generics
resource exhaustion
crafted dependency metadata
malformed object files
malicious compile-time code
```

Compiler crashes on ordinary invalid source are bugs.

---

# 74. Compiler Memory Safety

The DISP compiler itself should be implemented in a memory-safe language.

The initial implementation should prioritize:

```text
Rust
```

or DISP itself once sufficiently mature.

Unsafe compiler code should remain minimal.

---

# 75. Bootstrapping

Long-term goal:

```text
DISP compiler written in DISP
```

Possible stages:

```text
Stage 0:
compiler implemented in Rust

Stage 1:
basic DISP compiler

Stage 2:
DISP can compile compiler components

Stage 3:
self-hosting DISP compiler

Stage 4:
fully reproducible bootstrap
```

---

# 76. Self Hosting

DISP becomes self-hosting when the DISP compiler can compile its own source code.

This must not happen before DISP is sufficiently stable.

---

# 77. Debug Information

The compiler should emit debugging information compatible with platform debuggers.

Potential formats:

```text
DWARF
PDB
```

Source-level debugging must preserve DISP function names, variables, and line information.

---

# 78. Stack Traces

Runtime failures should provide useful stack traces when enabled.

Example:

```text
error: index 8 outside array length 4

at parse_user()
  src/parser.disp:81

at main()
  src/main.disp:12
```

---

# 79. Diagnostics

Compiler diagnostics are a first-class feature.

Every error should ideally include:

```text
what happened
where it happened
why it is invalid
what was expected
what was found
how to fix it
```

---

# 80. Error Example

Bad:

```text
Error E384.
```

Preferred:

```text
error: cannot use `buffer` after ownership moved

12 | let output = move buffer
                     ------ ownership moved here

15 | process(buffer)
             ^^^^^^ value no longer owned here

help: borrow the buffer instead if ownership transfer is not required
```

---

# 81. Diagnostic Stability

Diagnostic numeric codes may exist for documentation and tooling.

Human-readable explanations remain primary.

---

# 82. Compiler Recovery

The parser and semantic analyzer should recover from errors where possible.

One syntax mistake should not produce hundreds of meaningless secondary errors.

---

# 83. Linter

The compiler toolchain should include integrated linting.

Example:

```text
disp check
```

Possible warnings:

```text
unused variables
unused imports
unreachable code
unnecessary allocation
dangerous casts
deprecated API
suspicious concurrency
```

---

# 84. Security Lints

Security-oriented diagnostics should detect patterns such as:

```text
unchecked external input
unsafe memory operations
weak randomness
dangerous FFI
unbounded allocation
secret-dependent logging
insecure filesystem permissions
```

These should be carefully defined to avoid false guarantees.

---

# 85. Formatter

Official formatter:

```text
disp fmt
```

There should be one canonical style.

Projects should not need endless formatting configuration.

---

# 86. Documentation Generator

Documentation command:

```text
disp doc
```

Documentation should derive from:

```disp
/// documentation comments
```

Generated docs should understand:

```text
types
traits
functions
examples
modules
generic constraints
```

---

# 87. Test Compiler

Tests are compiled through the standard compiler pipeline.

Example:

```disp
@test
fn test_add() {
    assert(add(2, 2) == 4)
}
```

Command:

```text
disp test
```

---

# 88. Benchmarking

Command:

```text
disp bench
```

Benchmark tooling should support:

```text
warmup
iterations
statistics
comparison
regression detection
```

---

# 89. Build Profiles

Potential profiles:

```text
debug
release
size
realtime
embedded
web
gpu
```

Profiles modify compilation behavior, not core language meaning.

---

# 90. Linker

The compiler must support linking with:

```text
DISP libraries
system libraries
C libraries
native frameworks
object files
static libraries
shared libraries
```

---

# 91. C ABI

DISP should support a stable C interoperability layer.

Example:

```disp
extern "C" {
    fn puts(text: ptr<u8>) -> i32
}
```

FFI remains unsafe unless wrapped by a proven-safe abstraction.

---

# 92. ABI Stability

DISP's native ABI should not be considered stable until explicitly versioned.

External interoperability should initially prefer:

```text
C ABI
```

for long-term compatibility.

---

# 93. Link-Time Optimization

Release builds may support:

```text
LTO
ThinLTO
```

The compiler may optimize across module boundaries.

---

# 94. Dead Code Elimination

Unused code should not remain in release binaries unless required for reflection, export, or external linkage.

---

# 95. Binary Size

DISP should actively track binary-size regressions.

Performance must not automatically mean massive executables.

---

# 96. JIT Compilation

DISP may eventually support JIT execution for:

```text
REPL
notebooks
AI
dynamic workloads
database expressions
development
```

JIT is optional.

AOT remains the default native model.

---

# 97. REPL

Potential command:

```text
disp repl
```

Example:

```text
> let x = 10
> x * 2
20
```

The REPL must preserve DISP's normal type and safety rules.

---

# 98. Language Server

DISP should provide an official language server.

Capabilities:

```text
completion
diagnostics
go to definition
rename
references
hover
type information
formatting
refactoring
```

---

# 99. IDE Integration

The language server should enable consistent support across:

```text
VS Code
Zed
JetBrains
Neovim
Visual Studio
other editors
```

without each editor reimplementing the compiler.

---

# 100. Compiler API

The compiler architecture should expose reusable libraries for:

```text
parser
syntax tree
type checker
formatter
diagnostics
documentation
language server
```

The command-line compiler should not be one monolithic executable internally.

---

# 101. Compiler Modules

Initial compiler source architecture may look like:

```text
compiler/
├── lexer
├── parser
├── ast
├── resolve
├── types
├── ownership
├── borrow
├── effects
├── hir
├── mir
├── optimize
├── codegen
├── backend
├── linker
└── diagnostics
```

---

# 102. Frontend Boundary

The frontend is responsible for:

```text
parsing
name resolution
type checking
ownership
borrowing
semantic validation
```

No invalid safe DISP program should reach code generation.

---

# 103. Backend Boundary

Backends receive already validated IR.

Backends must not redefine DISP semantics.

The same valid program should have equivalent defined behavior across supported targets.

---

# 104. Verification

Critical compiler transformations should be tested aggressively.

Testing categories:

```text
unit tests
parser tests
type tests
compile-fail tests
runtime tests
fuzzing
property testing
differential testing
optimization tests
security tests
```

---

# 105. Fuzzing

Compiler components should be fuzzed.

Priority targets:

```text
lexer
parser
type checker
MIR optimizer
binary readers
package metadata
```

Malformed input must not compromise the compiler.

---

# 106. Differential Testing

Where possible, different compiler modes or backends should execute the same program and compare results.

This can detect optimization and backend bugs.

---

# 107. Compile-Fail Tests

Invalid DISP code should be tested intentionally.

Example:

```disp
let x: i32 = "hello"
```

The compiler test verifies that the correct error is emitted.

---

# 108. Optimization Verification

Every optimization must preserve defined behavior.

Optimization bugs are correctness bugs.

Safety checks may only be removed when the compiler proves they are unnecessary.

---

# 109. Supply-Chain Security

Compiler binaries should eventually support:

```text
signed releases
reproducible builds
verified dependencies
checksummed packages
build provenance
```

---

# 110. Package Security

During compilation, package dependencies must be validated.

The compiler or package manager should detect:

```text
hash mismatches
unexpected package changes
invalid signatures
dependency confusion
version conflicts
```

---

# 111. Sandboxed Build Scripts

If DISP allows package build scripts, they must not receive unrestricted system access by default.

Capabilities should be explicit.

Example permissions:

```text
read package files
write build output
network access
process execution
environment access
```

---

# 112. Cross Compilation

DISP should support:

```text
build machine != target machine
```

Example:

```text
disp build --target aarch64-linux
```

Cross compilation must not require running target code during ordinary compilation.

---

# 113. Target Descriptions

Compiler targets should describe:

```text
architecture
operating system
ABI
pointer width
endianness
CPU features
runtime availability
```

---

# 114. Feature Detection

DISP should support CPU feature selection.

Example:

```text
disp build --cpu native
```

or:

```text
disp build --features avx2
```

Portable binaries should have controlled fallback behavior.

---

# 115. Profile-Guided Optimization

Future support may include:

```text
PGO
```

Workflow:

```text
instrument
run real workload
collect profile
recompile
optimize hot paths
```

---

# 116. Runtime Dispatch

A binary may contain multiple optimized implementations.

Example:

```text
generic SIMD
AVX2
AVX-512
```

The runtime selects the best supported path.

This should be compiler-generated where useful.

---

# 117. AI-Assisted Diagnostics

DISP tooling may eventually provide optional intelligent explanations.

However:

```text
compiler correctness
```

must never depend on AI.

The compiler itself must remain deterministic and formally defined.

---

# 118. Compiler Performance Goal

The compiler itself must be fast.

Major goals:

```text
fast startup
parallel compilation
incremental compilation
efficient type checking
bounded generic analysis
low memory usage
```

Compile-time performance is a language feature.

---

# 119. Runtime Performance Goal

Generated programs should target performance competitive with leading native systems languages.

Performance must be demonstrated using real benchmarks.

No performance claim becomes part of DISP merely through design intent.

---

# 120. Safety Goal

Safe DISP machine code must preserve the guarantees established by the frontend.

The backend must never introduce:

```text
use-after-free
invalid bounds behavior
undefined arithmetic
broken drop logic
data races
```

for valid safe programs.

---

# 121. Compiler Correctness Rule

If:

```text
source semantics
```

and:

```text
generated machine behavior
```

disagree,

the compiler is wrong.

---

# 122. No Undefined Compiler Freedom

DISP should minimize language-level undefined behavior.

Optimizers may only assume guarantees explicitly defined by the language specification.

---

# 123. Internal Unsafe Code

Compiler implementation may require limited unsafe operations.

Every unsafe block should have:

```text
small scope
documented invariant
tests
review
```

Unsafe compiler code must not become the default implementation strategy.

---

# 124. Compiler Bootstrap Security

When DISP becomes self-hosting, compiler trust becomes critical.

Long-term reproducible bootstrap should allow validation that:

```text
compiler source
```

corresponds to:

```text
compiler binary
```

---

# 125. Architecture Summary

The initial DISP compiler architecture is:

```text
DISP Source
    ↓
Lexer
    ↓
Parser
    ↓
AST
    ↓
Resolver
    ↓
Type System
    ↓
Ownership + Borrow Checking
    ↓
Safety / Effect Analysis
    ↓
HIR
    ↓
MIR
    ↓
Optimization
    ↓
Backend
    ├── Native
    ├── WebAssembly
    ├── GPU
    └── Embedded
    ↓
Linking
    ↓
DISP Program
```

---

# 126. First Implementation Strategy

The first practical DISP compiler should focus on:

```text
1. Lexer
2. Parser
3. AST
4. Basic type checker
5. Functions and variables
6. Structs and enums
7. Ownership
8. Borrow checking
9. MIR
10. Native code generation
```

Do not attempt:

```text
Data
Intelligence
System
Page
GPU
full async
full package ecosystem
```

all at once.

The core language must work first.

---

# 127. Initial Backend Strategy

Recommended first path:

```text
DISP frontend
      ↓
DISP MIR
      ↓
LLVM IR
      ↓
LLVM
      ↓
native machine code
```

This allows the project to focus first on DISP semantics instead of immediately building a complete native optimizer and machine-code backend.

---

# 128. Long-Term Backend Strategy

DISP should remain capable of supporting:

```text
LLVM backend
DISP native backend
WebAssembly backend
GPU backend
embedded backend
```

without changing the language frontend.

---

# 129. Compiler Development Rule

Every compiler phase must have a clearly defined input and output.

No compiler subsystem should secretly depend on unrelated frontend or backend state.

This keeps the compiler:

```text
testable
replaceable
auditable
parallelizable
maintainable
```

---

# 130. DISP Compiler Principle

> The compiler carries complexity so DISP programmers do not have to.

And:

> Every optimization must be earned by proof, not assumption.

---

# DISP

**Data. Intelligence. System. Page.**

**Simple source. Deep verification. Fast machine code.**

---

# 131. Bootstrap IR Strategy

The bootstrap compiler lowers validated AST into a typed and resolved HIR, then into an
ownership-explicit MIR and control-flow graph. Semantic identities, types, static call
targets, generic substitutions, receiver modes, and source spans are fixed in HIR;
backend phases must not consult parser syntax or rerun name resolution.

Generics currently use generic HIR/MIR plus recorded concrete substitutions at call
sites. Monomorphization is the first responsibility of native backend preparation.
Constrained generic trait calls retain a resolved trait-and-method identity until that
monomorphization step; calls whose receiver is already concrete point directly to the
selected implementation function.

MIR represents moves, copies, shared and mutable borrows, raw dereferences, aggregate
construction, calls, drop flags, and reverse-order cleanup explicitly. Executable user
destructor bodies remain deferred, but MIR drop statements define their eventual call
sites without relying on interpreter teardown behavior.
