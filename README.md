# DISP

**DISP — Data Intelligence System Page** is an evolving programming language
designed around a simple goal: make ordinary programming easy, safe by default,
and capable of high-performance native execution.

The repository contains the language specifications, roadmap, Rust compiler,
native backend, interpreter, examples, fuzz targets, and test suites.

## Current implementation

The current compiler includes static typing, ownership and borrowing, native
code generation, algebraic data types, generics and traits, strings, slices,
lists, maps, sets, iteration, paths, filesystem operations, and time
foundations. The implementation remains under active development and should not
yet be treated as a stable production language.

See [DISP.md](DISP.md), [SPEC.md](SPEC.md), and [ROADMAP.md](ROADMAP.md) for the
vision, current specification, and planned development.

## Build and test

The compiler is a Rust crate in `compiler/`:

```sh
cd compiler
cargo build
cargo test -- --test-threads=1
```

Run a DISP example through native compilation:

```sh
cargo run -- run examples/easy_disp.disp
```

Run the same program through the interpreter:

```sh
cargo run -- interpret examples/easy_disp.disp
```
