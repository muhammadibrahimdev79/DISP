# DISP Domain Architecture

## 0. Status

This document defines how DISP unifies its four primary computing domains:

```text
Data
Intelligence
System
Page
```

These are not separate languages.

They share:

```text
one syntax
one type system
one memory model
one compiler
one package system
one security model
one toolchain
```

---

# 1. Core Principle

> Data, Intelligence, System, and Page are capabilities of one language.

DISP must never become four unrelated DSLs joined together artificially.

---

# 2. The Four Domains

```text
DISP
│
├── Data
│   ├── databases
│   ├── queries
│   ├── storage
│   ├── analytics
│   └── streaming
│
├── Intelligence
│   ├── AI
│   ├── machine learning
│   ├── tensors
│   ├── numerical computing
│   └── accelerators
│
├── System
│   ├── operating systems
│   ├── embedded
│   ├── networking
│   ├── hardware
│   └── native applications
│
└── Page
    ├── web
    ├── desktop UI
    ├── mobile UI
    ├── layout
    └── interaction
```

---

# 3. Shared Language

All domains use ordinary DISP constructs:

```disp
let
var
fn
struct
enum
trait
impl
match
async
await
Result
Option
```

Domain functionality extends the language without replacing its foundations.

---

# 4. Shared Types

A type defined once should work across domains.

```disp
struct User {
    id: UserID
    name: String
    active: bool
}
```

The same `User` may be used by:

```text
database queries
AI processing
system services
Page components
network APIs
```

No duplicate model definitions should be required.

---

# 5. Shared Security

All domains inherit DISP's security guarantees.

These include:

```text
memory safety
type safety
bounds safety
null safety
ownership safety
safe concurrency
explicit unsafe boundaries
capability control
```

---

# 6. Shared Error Model

Recoverable failures use:

```disp
Result<T, E>
```

Missing values use:

```disp
Option<T>
```

This applies across all four domains.

---

# DATA

# 7. Data Goal

The Data domain replaces the need to constantly switch between:

```text
application language
SQL
database libraries
serialization frameworks
analytics languages
```

while preserving access to underlying database capabilities.

---

# 8. Data Definitions

Conceptual model:

```disp
data User {
    id: u64 primary
    name: String
    email: String unique
    active: bool
}
```

Data definitions are statically typed.

---

# 9. Queries

```disp
let users =
    User
    .where(active == true)
    .select(id, name)
```

The compiler must understand the result type.

---

# 10. Query Safety

This should fail at compile time:

```disp
User.select(field_that_does_not_exist)
```

when the schema is known.

---

# 11. Typed Parameters

External values remain parameters rather than query syntax.

```disp
let user =
    User
    .where(id == requested_id)
    .first()
```

This should eliminate ordinary SQL injection opportunities.

---

# 12. Insert

```disp
User.insert {
    name: "Alice"
    email: "alice@example.com"
    active: true
}
```

---

# 13. Update

```disp
User
    .where(id == user_id)
    .update {
        active: false
    }
```

---

# 14. Delete

```disp
User
    .where(id == user_id)
    .delete()
```

---

# 15. Transactions

```disp
transaction db {
    account_a.balance -= 100
    account_b.balance += 100
}
```

The transaction must either:

```text
commit completely
```

or:

```text
roll back
```

---

# 16. Raw SQL

DISP should still permit raw SQL when necessary.

```disp
db.query(
    "SELECT name FROM users WHERE id = ?",
    [id]
)
```

Raw access must not require unsafe string concatenation.

---

# 17. Database Portability

The Data system may support:

```text
PostgreSQL
SQLite
MySQL-compatible databases
distributed databases
embedded stores
```

Database-specific capabilities may still be exposed explicitly.

---

# 18. Schema Changes

Schema migrations should be first-class.

Conceptually:

```disp
migration AddUserStatus {
    add User.status: Status
}
```

Exact syntax remains provisional.

---

# 19. Analytics

Data operations should support pipelines:

```disp
let result =
    records
    .filter(valid)
    .group_by(category)
    .aggregate(sum(value))
```

---

# 20. Columnar Processing

Large analytical workloads should support efficient:

```text
columnar memory
SIMD
parallel execution
zero-copy operations
```

---

# 21. Streaming Data

Conceptual example:

```disp
async for event in stream {
    process(event)
}
```

Streams must support:

```text
backpressure
cancellation
bounded buffering
errors
```

---

# INTELLIGENCE

# 22. Intelligence Goal

The Intelligence domain should make DISP suitable for:

```text
AI
machine learning
scientific computing
numerical computing
GPU computing
large-scale tensor workloads
```

without requiring Python as the orchestration language.

---

# 23. Tensor Type

```disp
let tensor =
    Tensor<f32>.zeros([1024, 1024])
```

---

# 24. Static Shapes

When dimensions are known:

```disp
Tensor<f32, [32, 128]>
```

the compiler may include them in the type.

---

# 25. Shape Safety

Invalid operations should fail before execution when shape incompatibility can be proven.

Example:

```text
[32,128] × [64,32]
```

must not silently become a runtime bug.

---

# 26. Tensor Operations

Core operations may include:

```text
add
subtract
multiply
divide
matmul
transpose
reshape
reduce
softmax
convolution
normalization
```

---

# 27. Automatic Differentiation

DISP should support automatic differentiation.

Conceptually:

```disp
let gradients = grad(loss)
```

Autodiff must integrate with normal DISP functions.

---

# 28. Model Definition

Models should use normal DISP types.

```disp
struct Network {
    layer1: Linear
    layer2: Linear
}

impl Network {
    fn forward(self: &Self, x: Tensor<f32>) -> Tensor<f32> {
        ...
    }
}
```

No unrelated model-definition language should be required.

---

# 29. Training

Conceptually:

```disp
for batch in dataset {
    let output = model.forward(batch.input)
    let loss = loss_fn(output, batch.target)

    optimizer.step(grad(loss))
}
```

---

# 30. Devices

Common device types:

```text
CPU
GPU
Accelerator
```

Example:

```disp
let device = Device.best()
```

---

# 31. Device Placement

```disp
let tensor = tensor.to(GPU)
```

Expensive transfers must not be hidden without compiler justification.

---

# 32. GPU Kernels

Low-level acceleration:

```disp
gpu fn add(
    a: Slice<f32>,
    b: Slice<f32>,
    out: MutSlice<f32>
) {
    ...
}
```

---

# 33. GPU Safety

GPU code should preserve:

```text
bounds safety
typed buffers
ownership
lifetime correctness
synchronization guarantees
```

where possible.

---

# 34. Accelerator Portability

DISP should avoid tying ordinary Intelligence code permanently to one vendor.

Potential backends include:

```text
CUDA/PTX
SPIR-V
WebGPU
Metal
other accelerator APIs
```

---

# 35. SIMD

CPU numerical code should support automatic and explicit SIMD.

```disp
let values: simd<f32, 8>
```

---

# 36. Parallel Intelligence

Parallel numerical execution should integrate with safe concurrency.

```disp
parallel for item in dataset {
    process(item)
}
```

---

# 37. AI Model Security

Model files are untrusted external input.

Loading them must not automatically execute arbitrary host code.

---

# 38. Intelligence Memory

Large tensor workloads require explicit awareness of:

```text
device memory
host memory
pinned memory
shared memory
memory pools
```

while keeping ordinary APIs simple.

---

# SYSTEM

# 39. System Goal

The System domain makes DISP suitable for replacing major workloads currently written in:

```text
C
C++
Rust
```

while keeping safe defaults.

---

# 40. System Applications

DISP should support:

```text
operating systems
kernels
drivers
firmware
embedded systems
servers
game engines
databases
network stacks
command-line tools
native libraries
high-performance services
```

---

# 41. Native Compilation

System code should compile directly to native machine code.

Primary targets include:

```text
x86-64
ARM64
RISC-V
```

---

# 42. No Mandatory VM

Native System programs must not require:

```text
JVM
browser
JavaScript runtime
heavy virtual machine
mandatory garbage collector
```

---

# 43. Freestanding Programs

DISP must support environments with:

```text
no operating system
no heap
no filesystem
no networking
no standard runtime
```

---

# 44. Raw Pointers

Raw pointers exist for legitimate low-level programming.

```disp
ptr<T>
mut ptr<T>
```

Dereferencing requires:

```disp
unsafe {
    ...
}
```

---

# 45. Hardware Access

Conceptual example:

```disp
unsafe {
    let register =
        mut ptr<u32>(0x4000_0000)

    *register = 1
}
```

Dangerous operations must look dangerous.

---

# 46. Memory Layout

System types may define ABI-compatible layouts.

```disp
@repr(C)
struct Header {
    kind: u16
    size: u32
}
```

---

# 47. Alignment

```disp
@align(64)
struct CacheLine {
    ...
}
```

System programmers need predictable memory layout when explicitly requested.

---

# 48. Custom Allocators

DISP must support:

```text
arena allocators
pool allocators
bump allocators
system allocators
kernel allocators
custom allocators
```

---

# 49. No-Heap Programming

Fixed-capacity types should support systems that cannot allocate dynamically.

Examples:

```text
FixedList<T, N>
FixedString<N>
FixedMap<K, V, N>
```

---

# 50. Deterministic Cleanup

System resources must support deterministic destruction.

Examples:

```text
memory
files
sockets
locks
hardware handles
GPU resources
```

---

# 51. FFI

DISP should interoperate with existing native ecosystems.

```disp
extern "C" {
    fn puts(text: ptr<u8>) -> i32
}
```

Foreign calls remain security boundaries.

---

# 52. C ABI

C ABI should be the initial universal native interoperability standard.

This enables integration with:

```text
operating systems
C libraries
C++
Rust
hardware SDKs
existing native ecosystems
```

---

# 53. System Calls

Operating-system APIs may be wrapped safely.

Low-level direct system calls should remain available for advanced environments.

---

# 54. Networking

System-level networking should expose:

```text
TCP
UDP
sockets
raw networking where permitted
async I/O
zero-copy buffers
```

---

# 55. Async I/O

DISP should map to efficient platform mechanisms such as:

```text
io_uring
epoll
kqueue
IOCP
```

without exposing platform complexity to ordinary code.

---

# 56. Threads

```disp
let worker = Thread.spawn {
    work()
}
```

Safe DISP must prevent data races.

---

# 57. Atomics

Low-level concurrency requires:

```text
AtomicBool
AtomicI32
AtomicU64
AtomicPtr<T>
```

with explicitly defined memory ordering.

---

# 58. Real-Time Systems

DISP must support workloads where unpredictable pauses are unacceptable.

Real-time profiles should allow:

```text
no GC
controlled allocation
bounded queues
deterministic cleanup
controlled synchronization
```

---

# 59. Embedded

Embedded DISP should support:

```text
microcontrollers
firmware
bare metal
limited RAM
limited flash
interrupt-driven systems
```

---

# 60. Interrupts

Hardware interrupt handling requires special compiler/runtime rules.

Conceptually:

```disp
@interrupt
fn timer_interrupt() {
    ...
}
```

Exact syntax remains provisional.

---

# 61. Volatile Memory

Hardware registers require explicit volatile operations.

Conceptually:

```disp
unsafe {
    volatile.write(register, value)
}
```

Ordinary memory access must not silently become volatile.

---

# 62. Assembly

Inline or external assembly may eventually be supported.

Conceptually:

```disp
unsafe {
    asm(...)
}
```

Assembly is inherently architecture-specific and unsafe.

---

# 63. Kernel Development

Kernel profiles should support:

```text
custom entry point
custom panic handler
no standard allocator
interrupts
memory mapping
page tables
hardware access
```

---

# 64. System Security

Safe abstractions should encapsulate unsafe hardware and OS boundaries.

The amount of trusted unsafe System code should remain minimal.

---

# PAGE

# 65. Page Goal

The Page domain aims to replace the need to combine:

```text
HTML
CSS
JavaScript
TypeScript
multiple frontend frameworks
```

for ordinary interface development.

---

# 66. Pages

```disp
page Home {
    text("Hello, DISP!")
}
```

---

# 67. Components

```disp
component UserCard(user: User) {
    Column {
        text(user.name)
        text(user.email)
    }
}
```

Components are normal typed DISP functions/components.

---

# 68. Layout

Core primitives may include:

```text
Row
Column
Grid
Stack
Scroll
Container
```

Example:

```disp
Column {
    text("Welcome")
    button("Continue")
}
```

---

# 69. Styling

```disp
style UserCard {
    width: 100%
    padding: 16px
    radius: 12px
}
```

Style values should be typed.

---

# 70. Typed Styling

Invalid combinations should be caught early.

For example:

```text
width
```

expects a dimensional value rather than arbitrary text.

---

# 71. Responsive Design

Conceptual syntax:

```disp
when width < 600px {
    ...
}
```

Responsive behavior remains part of the Page type system.

---

# 72. State

```disp
state count = 0
```

State is statically typed.

---

# 73. Events

```disp
button("Count: {count}") {
    on click {
        count += 1
    }
}
```

Event payloads must also be typed.

---

# 74. Inputs

```disp
input {
    value: username

    on change(value) {
        username = value
    }
}
```

---

# 75. Routing

```disp
route "/" -> Home
route "/login" -> Login
route "/users/{id}" -> UserPage
```

Route parameters should be typed.

---

# 76. Backend Routes

The same language may define server endpoints.

```disp
route GET "/api/users/{id}" {
    let user = find_user(id)?
    return user
}
```

---

# 77. Full-Stack Type Sharing

A type should be defined once:

```disp
struct User {
    id: UserID
    name: String
}
```

and reused by:

```text
database
server
serialization
client
Page
AI
```

---

# 78. Serialization

Server-to-client values should use compiler-verified serialization.

```disp
return user
```

The compiler/runtime handles the supported wire representation.

---

# 79. HTML Safety

Ordinary text must be escaped automatically.

```disp
text(user_input)
```

must not interpret arbitrary user input as executable markup.

---

# 80. Raw Markup

Raw markup requires explicit intent.

Conceptually:

```disp
unsafe_html(value)
```

---

# 81. Browser Target

Page code may compile to:

```text
WebAssembly
browser-compatible generated code
native browser bindings
```

The implementation strategy may evolve.

---

# 82. JavaScript Interoperability

Existing browser ecosystems may require JS interoperability.

This should remain an interop boundary rather than making JS semantics part of DISP.

---

# 83. Server-Side Rendering

DISP Page should support:

```text
SSR
static generation
client rendering
hydration
```

where appropriate.

---

# 84. Desktop

The Page model should eventually support native desktop interfaces.

Potential targets:

```text
Windows
macOS
Linux
```

---

# 85. Mobile

Long-term Page targets may include:

```text
Android
iOS
```

without requiring a separate application language.

---

# 86. Accessibility

Accessibility must be first-class.

The Page compiler/linter should assist with:

```text
semantic roles
labels
keyboard navigation
focus
screen readers
contrast
```

---

# UNIFICATION

# 87. Data + Page

Example:

```disp
page Users {
    let users =
        await User
        .where(active == true)
        .select(id, name)

    Column {
        for user in users {
            text(user.name)
        }
    }
}
```

The same compiler understands the query and UI types.

---

# 88. Data + Intelligence

```disp
let records =
    TrainingData
    .where(valid == true)
    .select(features, label)

let model = train(records)
```

Data should flow directly into Intelligence workloads without unnecessary serialization boundaries.

---

# 89. Intelligence + Page

```disp
page Classifier {
    state result: Option<Prediction> = None

    button("Analyze") {
        on click {
            result = Some(await model.predict(input))
        }
    }
}
```

---

# 90. System + Intelligence

```disp
let device = gpu.device(0)?
let model = Model.load(device, "model.dispmodel")?
```

Low-level hardware access and high-level AI remain part of one language.

---

# 91. System + Data

High-performance databases may use:

```text
custom memory
SIMD
async networking
storage engines
typed queries
```

without crossing into another language.

---

# 92. System + Page

Native applications may combine:

```text
native system APIs
filesystem
networking
Page UI
```

inside one application.

---

# 93. Full DISP Example

```disp
data User {
    id: u64 primary
    name: String
    score: f32
}

fn classify(user: &User) -> Category {
    model.predict(user.score)
}

route GET "/api/users" {
    return User
        .select(id, name, score)
}

page Home {
    state users =
        await fetch<List<User>>("/api/users")

    Column {
        text("DISP Users")

        for user in users {
            let category = classify(&user)

            Row {
                text(user.name)
                text("{category}")
            }
        }
    }
}

style Home {
    width: 100%
    padding: 24px
}

fn main() {
    run(Home)
}
```

This combines:

```text
Data
+
Intelligence
+
System runtime
+
Page
```

within one language.

---

# 94. Domain Boundaries

Domain-specific behavior should still be explicit when it has meaningful cost or restrictions.

Examples:

```text
database query
GPU transfer
network access
unsafe hardware access
Page rendering
```

DISP must not hide major costs merely for aesthetic simplicity.

---

# 95. Domain Capabilities

Applications may explicitly declare required capabilities.

Conceptually:

```text
database
network
filesystem
GPU
Page
device
```

This enables stronger sandboxing and deployment analysis.

---

# 96. Compile-Time Validation

The compiler should validate domain-specific rules before execution wherever practical.

Examples:

```text
invalid database field
invalid tensor shape
invalid GPU operation
invalid hardware access type
invalid Page property
invalid event payload
```

---

# 97. Domain Optimization

Because all four domains share one compiler, DISP can optimize across boundaries.

Potential examples:

```text
query pushdown
zero-copy database decoding
tensor fusion
GPU kernel fusion
server/client serialization elimination
Page dependency analysis
SIMD specialization
```

---

# 98. Zero-Copy Goal

Data should not be copied merely because it moves between DISP domains.

The compiler/runtime should use:

```text
borrowing
views
shared buffers
memory mapping
device-aware buffers
```

where safe and beneficial.

---

# 99. Unified Async Model

Database operations:

```disp
await query()
```

Network operations:

```disp
await fetch()
```

GPU operations:

```disp
await gpu.execute()
```

Page operations:

```disp
await load()
```

should share one coherent async model.

---

# 100. Unified Package Model

Domain libraries use the same:

```text
DISP.toml
DISP.lock
disp add
disp build
disp test
```

There must not be separate incompatible package managers for each domain.

---

# 101. Unified Testing

The same test framework should cover:

```text
Data
Intelligence
System
Page
```

Example:

```disp
@test
fn test_user_query() {
    ...
}

@test
fn test_model() {
    ...
}

@test
fn test_allocator() {
    ...
}

@test
fn test_page() {
    ...
}
```

---

# 102. Unified Debugging

The debugger should eventually understand:

```text
native stack frames
async tasks
database queries
GPU execution
Page components
```

through one DISP development experience.

---

# 103. Unified Profiling

DISP profiling should eventually show:

```text
CPU time
allocation
I/O
database queries
GPU kernels
network latency
Page rendering
```

in one performance model.

---

# 104. Domain Independence

Applications do not need all four domains.

A kernel may use only:

```text
System
```

An AI worker may use:

```text
Intelligence + System
```

A website may use:

```text
Data + Page
```

Unused domains must not impose runtime or binary costs.

---

# 105. Pay-for-What-You-Use

If a program does not use:

```text
Data
Intelligence
Page
```

the related runtime systems must not be linked.

The same applies to System features not required by the application.

---

# 106. No Domain Privilege

No domain may bypass the core type or security system.

For example:

```text
GPU code
database code
Page code
system code
```

must all respect defined DISP safety rules unless explicitly inside an unsafe boundary.

---

# 107. Domain Syntax Rule

Domain syntax may introduce convenient constructs only when they:

```text
remain parseable
remain type-safe
have clear semantics
integrate with normal DISP
```

Domain syntax must not become arbitrary embedded text languages.

---

# 108. Escape Hatches

Experts still need direct access.

Therefore DISP may permit:

```text
raw SQL
raw GPU kernels
raw pointers
FFI
native browser APIs
platform-specific APIs
```

but these should be clearly separated from ordinary safe abstractions.

---

# 109. Performance Principle

DISP must pursue performance across every domain.

Targets:

```text
System -> native systems-language performance
Intelligence -> accelerator-class performance
Data -> efficient compiled queries and data pipelines
Page -> minimal rendering and runtime overhead
```

These are goals that must be validated through benchmarks.

---

# 110. Simplicity Principle

A beginner should still be able to write:

```disp
fn main() {
    print("Hello, DISP!")
}
```

without learning any domain system.

Complexity appears only when needed.

---

# 111. Security Principle

Every domain must follow:

```text
safe defaults
least privilege
typed boundaries
validated external data
bounded resource usage
explicit dangerous operations
```

---

# 112. Architecture Summary

DISP's four-domain architecture is:

```text
                 DISP
                  │
        ┌─────────┼─────────┐
        │         │         │
      Data   Intelligence  System
        │         │         │
        └─────────┼─────────┘
                  │
                 Page
                  │
        Shared DISP Foundations
                  │
        ├── Syntax
        ├── Types
        ├── Memory
        ├── Security
        ├── Compiler
        ├── Runtime
        └── Packages
```

All four domains are peers built on the same foundations.

---

# 113. Implementation Order

The domains must not all be implemented simultaneously.

Recommended order:

```text
1. Core DISP language
2. System foundations
3. Data
4. Intelligence
5. Page
6. Deep cross-domain optimization
```

System foundations come early because every other domain ultimately depends on native execution, memory, concurrency, and runtime infrastructure.

---

# 114. Stability Rule

A domain feature becomes stable only after:

```text
semantics are defined
type checking exists
security is reviewed
compiler support exists
tests exist
real applications use it
performance is measured
```

---

# 115. DISP Domain Rule

> One language does not mean hiding every distinction.

DISP should unify concepts where unification improves programming.

It should preserve meaningful distinctions where hardware, security, performance, or semantics require them.

---

# 116. DISP Principle

> Data handles information.

> Intelligence transforms information.

> System controls computation.

> Page presents computation.

Together:

# DISP

**Data. Intelligence. System. Page.**

**One language for the full computing stack.**