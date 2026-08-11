# DISP

## Data · Intelligence · System · Page

**DISP** is a next-generation, general-purpose programming language designed around four fundamental computing domains:

* **Data** — databases, storage, analytics, serialization, and data processing.
* **Intelligence** — artificial intelligence, machine learning, numerical computing, accelerators, and GPU workloads.
* **System** — operating systems, applications, servers, embedded software, networking, and performance-critical computing.
* **Page** — web applications, user interfaces, layouts, styling, and interactive experiences.

## Mission

DISP aims to become a single coherent language capable of handling workloads that currently require multiple languages such as Python, Mojo, C, C++, Rust, JavaScript, TypeScript, HTML, CSS, and SQL.

DISP does not attempt to combine their syntax.

Instead, DISP will design unified abstractions for their capabilities from first principles.

## Core Goals

### 1. Fast

DISP targets native, predictable performance.

The language should support:

* Ahead-of-time compilation
* Zero-cost abstractions
* Efficient memory layouts
* SIMD
* Multithreading
* Asynchronous execution
* GPU and accelerator computing
* Explicit low-level control when required
* Minimal runtime overhead

### 2. Easy

Common programs should remain simple.

DISP should provide:

* Clean and consistent syntax
* Strong type inference
* Useful compiler diagnostics
* Automatic resource management where safe
* First-class package management
* Integrated formatting
* Integrated testing
* Integrated documentation
* One standard toolchain

Complexity should be introduced only when the programmer actually needs it.

### 3. Secure

Safety is the default.

DISP should pursue:

* Memory safety
* Type safety
* Bounds safety
* Overflow-aware arithmetic
* Safe concurrency
* Explicit capability boundaries
* Secure dependency management
* No undefined behavior in safe DISP
* Explicitly isolated low-level unsafe operations
* Compiler-enforced security guarantees where practical

Unsafe operations must never silently weaken safe code.

### 4. All-Purpose

DISP is intended to support:

* Systems programming
* Command-line applications
* Desktop applications
* Mobile applications
* Web frontend
* Web backend
* APIs
* Databases
* Distributed systems
* Cloud infrastructure
* Embedded systems
* Game development
* Scientific computing
* AI and machine learning
* GPU computing
* Automation and scripting

## The Four Domains

```text
DISP
│
├── Data
│   ├── Database
│   ├── Query
│   ├── Storage
│   └── Analytics
│
├── Intelligence
│   ├── AI
│   ├── ML
│   ├── Numerical Computing
│   └── GPU / Accelerators
│
├── System
│   ├── Native Applications
│   ├── Operating Systems
│   ├── Embedded
│   └── Servers
│
└── Page
    ├── UI
    ├── Layout
    ├── Styling
    └── Interaction
```

These are not separate languages.

They are capabilities of **one language, one type system, one compiler architecture, and one ecosystem.**

## Design Principle

> **Easy by default. Powerful when necessary. Safe by default. Fast everywhere.**

DISP should not sacrifice correctness for convenience or performance for abstraction.

When trade-offs are unavoidable, they must be explicit and measurable.

## Non-Goals

DISP will not become a collection of unrelated mini-languages.

DISP will not blindly copy existing language syntax.

DISP will not claim performance or security properties that cannot be demonstrated.

DISP will not hide expensive operations behind apparently cheap abstractions without making their cost understandable.

DISP will not make unsafe behavior the normal path.

## Toolchain Vision

The DISP ecosystem should ultimately provide a unified command:

```bash
disp
```

with integrated functionality such as:

```bash
disp new
disp build
disp run
disp test
disp check
disp fmt
disp doc
disp bench
disp package
```

## Long-Term Standard

Every major DISP feature should be judged against four questions:

1. **Does it make DISP easier?**
2. **Does it preserve or improve performance?**
3. **Does it preserve DISP's safety guarantees?**
4. **Does it strengthen DISP as a general-purpose language?**

If a feature fails these principles without compelling justification, it does not belong in DISP.

---

# DISP

**Data. Intelligence. System. Page.**

**One language for computing.**
